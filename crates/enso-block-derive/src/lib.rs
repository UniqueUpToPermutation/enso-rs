//! `#[derive(Ports)]`: generates `enso_block::Ports::register_ports` from a
//! block struct's annotated fields. Documented on the re-export,
//! `enso_block::Ports`.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as Tokens};
use quote::{quote, quote_spanned};
use syn::{
    Data, DeriveInput, Expr, Field, Fields, Ident, LitInt, LitStr, Type, parse_macro_input, spanned::Spanned,
};

#[proc_macro_derive(Ports, attributes(input, output, inbox, outbox, child, private))]
pub fn derive_ports(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(&input) {
        Ok(tokens) => tokens.into(),
        // Errors become `compile_error!`s pointing at the offending code.
        Err(err) => err.to_compile_error().into(),
    }
}

/// What a field's attribute asks for.
enum Kind {
    Input { default: Option<Expr>, merge: Option<Ident> },
    Output { initial: Option<Expr> },
    Inbox { capacity: Option<Expr>, latest: bool },
    Outbox,
    Child { init: Option<Expr> },
}

struct PortField {
    ident: Ident,
    name: String,
    /// `id = N`: an explicit key.
    id: Option<LitInt>,
    kind: Kind,
    private: bool,
    span: Span,
}

fn expand(input: &DeriveInput) -> syn::Result<Tokens> {
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => fields.named.iter().collect::<Vec<_>>(),
            Fields::Unit => Vec::new(),
            Fields::Unnamed(_) => {
                return Err(syn::Error::new(input.span(), "#[derive(Ports)] needs named fields (or none)"));
            }
        },
        _ => return Err(syn::Error::new(input.span(), "#[derive(Ports)] works on structs only")),
    };

    // Report every bad field at once, not just the first.
    let mut ports = Vec::new();
    let mut errors: Option<syn::Error> = None;
    for field in fields {
        match parse_field(field) {
            Ok(Some(port)) => ports.push(port),
            Ok(None) => {}
            Err(err) => match &mut errors {
                Some(errors) => errors.combine(err),
                None => errors = Some(err),
            },
        }
    }
    if let Some(errors) = errors {
        return Err(errors);
    }

    let statements = ports.iter().map(|port| {
        let PortField { ident, name, span, id, .. } = port;
        // Identity: the explicit id, or a hash of the *field* name (not the
        // label), so relabelling never breaks saved data.
        let field = ident.to_string();
        let key = match id {
            Some(id) => quote! { ::enso_block::Key::new(#id) },
            None => quote! { ::enso_block::Key::named(#field) },
        };
        let spec = quote! { ::enso_block::Spec::new(#key, #name) };
        let register = match &port.kind {
            Kind::Input { default, merge: None } => {
                let default = or_default(default);
                quote_spanned! {*span=> self.#ident = r.input(#spec, #default); }
            }
            Kind::Input { default, merge: Some(mode) } => {
                let default = or_default(default);
                quote_spanned! {*span=>
                    self.#ident = r.input_merged(#spec, #default, ::enso_block::MergeMode::#mode);
                }
            }
            Kind::Output { initial } => {
                let initial = or_default(initial);
                quote_spanned! {*span=> self.#ident = r.output(#spec, #initial); }
            }
            Kind::Inbox { capacity, latest } => {
                let delivery = match (capacity, latest) {
                    (_, true) => quote! { ::enso_block::Delivery::Latest },
                    (Some(capacity), false) => quote! { ::enso_block::Delivery::Queue(#capacity) },
                    (None, false) => quote! { ::core::default::Default::default() },
                };
                quote_spanned! {*span=> self.#ident = r.inbox(#spec, #delivery); }
            }
            Kind::Outbox => quote_spanned! {*span=> self.#ident = r.outbox(#spec); },
            Kind::Child { init } => {
                let init = or_default(init);
                quote_spanned! {*span=> self.#ident = r.child(#spec, #init); }
            }
        };
        let private = port.private.then(|| quote_spanned! {*span=> r.private(self.#ident); });
        quote! { #register #private }
    });

    let ident = &input.ident;
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics ::enso_block::Ports for #ident #type_generics #where_clause {
            fn register_ports(&mut self, r: &mut ::enso_block::Registrar<'_>) {
                let _ = &r; // silence "unused" for blocks without annotated fields
                #(#statements)*
            }
        }
    })
}

/// `default = expr` if given, else the type's `Default`.
fn or_default(expr: &Option<Expr>) -> Tokens {
    match expr {
        Some(expr) => quote! { #expr },
        None => quote! { ::core::default::Default::default() },
    }
}

/// Read a field's `#[input(..)]` / `#[output(..)]` / `#[child(..)]` /
/// `#[private]`. `None` for fields without any (plain block state).
fn parse_field(field: &Field) -> syn::Result<Option<PortField>> {
    let ident = field.ident.clone().expect("named fields");
    let mut name = ident.to_string();
    let mut id = None;
    let mut kind = None;
    let mut private = false;

    for attr in &field.attrs {
        if attr.path().is_ident("private") {
            private = true;
            continue;
        }
        let Some(which) = ["input", "output", "inbox", "outbox", "child"].into_iter().find(|w| attr.path().is_ident(w))
        else {
            continue;
        };
        if kind.is_some() {
            return Err(syn::Error::new(attr.span(), "a field can be only one of input, output, inbox, outbox or child"));
        }

        let (mut value, mut merge, mut latest) = (None, None, false);
        // `#[input]` alone has no arguments; `#[input(..)]` has some.
        if matches!(attr.meta, syn::Meta::List(_)) {
            attr.parse_nested_meta(|meta| {
                let key = meta.path.get_ident().map(Ident::to_string).unwrap_or_default();
                match (which, key.as_str()) {
                    (_, "name") => name = meta.value()?.parse::<LitStr>()?.value(),
                    (_, "id") => id = Some(meta.value()?.parse::<LitInt>()?),
                    ("input", "default") | ("output", "initial") | ("child", "init") | ("inbox", "capacity") => {
                        value = Some(meta.value()?.parse::<Expr>()?);
                    }
                    ("inbox", "latest") => latest = true,
                    ("input", "merge") => {
                        let mode: Ident = meta.value()?.parse()?;
                        if !["Single", "Append", "Sum"].contains(&mode.to_string().as_str()) {
                            return Err(meta.error("merge must be Single, Append or Sum"));
                        }
                        merge = Some(mode);
                    }
                    _ => {
                        let allowed = match which {
                            "input" => "name, id, default, merge",
                            "output" => "name, id, initial",
                            "inbox" => "name, id, capacity, latest",
                            "outbox" => "name, id",
                            _ => "name, id, init",
                        };
                        return Err(meta.error(format!("unknown argument for #[{which}]; expected {allowed}")));
                    }
                }
                Ok(())
            })?;
        }

        // Catch "wrong handle type" here, with a clear message, rather than
        // as a type error inside generated code.
        let expected = match which {
            "input" => "In",
            "output" => "Out",
            "inbox" => "Inbox",
            "outbox" => "Outbox",
            _ => "Child",
        };
        if outer_type_name(&field.ty).as_deref() != Some(expected) {
            return Err(syn::Error::new(field.ty.span(), format!("#[{which}] fields must have type {expected}<..>")));
        }
        if latest && value.is_some() {
            return Err(syn::Error::new(attr.span(), "#[inbox] takes either `capacity` or `latest`, not both"));
        }

        kind = Some(match which {
            "input" => Kind::Input { default: value, merge },
            "output" => Kind::Output { initial: value },
            "inbox" => Kind::Inbox { capacity: value, latest },
            "outbox" => Kind::Outbox,
            _ => Kind::Child { init: value },
        });
    }

    match kind {
        Some(Kind::Child { .. }) if private => {
            Err(syn::Error::new(field.span(), "#[private] goes with ports and mailboxes, not children"))
        }
        Some(kind) => Ok(Some(PortField { ident, name, id, kind, private, span: field.span() })),
        None if private => Err(syn::Error::new(field.span(), "#[private] goes with a port or mailbox attribute")),
        None => Ok(None),
    }
}

/// `In` for `In<f32>` or `enso_block::In<f32>`.
fn outer_type_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(path) => path.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    }
}
