//! Test-only source census. No Core dependency and no token-string type comparison.

use std::collections::{BTreeMap, BTreeSet};

use syn::parse::{Parse, ParseStream};
use syn::{
    braced, FnArg, GenericArgument, Item, Pat, PathArguments, ReceiverKind, ReturnType, Token,
    TraitItem, Type,
};

pub const NATIVE_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../src/browser/backend.rs"
));
pub const OPERATIONS_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/browser/operations.rs"
));

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    Named(String, Vec<Shape>),
    Tuple(Vec<Shape>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub parameters: Vec<(String, Shape)>,
    pub success: Shape,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub method: String,
    pub variant: String,
    pub signature: Signature,
}

pub type Methods = BTreeMap<String, Signature>;

fn named(name: &str, args: Vec<Shape>) -> Shape {
    Shape::Named(name.into(), args)
}

// Preserve the AST structure, including generic and tuple order. Qualified paths,
// associated bindings, lifetimes, functions, arrays, pointers, etc. fail closed.
fn owned_shape(ty: &Type) -> Result<Shape, String> {
    match ty {
        Type::Path(path)
            if path.qself.is_none()
                && path.path.leading_colon.is_none()
                && path.path.segments.len() == 1 =>
        {
            let segment = &path.path.segments[0];
            let args = match &segment.arguments {
                PathArguments::None => Vec::new(),
                PathArguments::AngleBracketed(args) if args.colon2_token.is_none() => args
                    .args
                    .iter()
                    .map(|arg| match arg {
                        GenericArgument::Type(ty) => owned_shape(ty),
                        _ => Err("unsupported generic argument".into()),
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                _ => return Err("unsupported path arguments".into()),
            };
            Ok(named(&segment.ident.to_string(), args))
        }
        Type::Tuple(tuple) => Ok(Shape::Tuple(
            tuple
                .elems
                .iter()
                .map(owned_shape)
                .collect::<Result<_, _>>()?,
        )),
        _ => Err("unsupported owned type syntax".into()),
    }
}

// These are ownership conversions, not a method inventory. Only the borrowed
// forms actually present in the native contract are accepted.
fn input_shape(ty: &Type) -> Result<Shape, String> {
    if let Type::Reference(reference) = ty {
        if reference.mutability.is_some() || reference.lifetime.is_some() {
            return Err("unsupported mutable or explicitly lifetime-bound input".into());
        }
        if let Type::Slice(slice) = &*reference.elem {
            let element = owned_shape(&slice.elem)?;
            let string = named("String", vec![]);
            let pair = Shape::Tuple(vec![named("ActionTarget", vec![]), string.clone()]);
            if element != string && element != pair {
                return Err("unsupported borrowed slice conversion".into());
            }
            return Ok(named("Vec", vec![element]));
        }
        return match owned_shape(&reference.elem)? {
            Shape::Named(name, args) if args.is_empty() => match name.as_str() {
                "str" => Ok(named("String", vec![])),
                "Path" => Ok(named("PathBuf", vec![])),
                "WaitCondition" | "EmulateOptions" | "CookieOp" => Ok(named(&name, vec![])),
                _ => Err(format!("unsupported borrowed input {name}")),
            },
            _ => Err("unsupported borrowed input shape".into()),
        };
    }
    if let Type::Path(path) = ty {
        if path.qself.is_none()
            && path.path.leading_colon.is_none()
            && path.path.segments.len() == 1
            && path.path.segments[0].ident == "Option"
        {
            if let PathArguments::AngleBracketed(args) = &path.path.segments[0].arguments {
                if args.colon2_token.is_none() && args.args.len() == 1 {
                    if let GenericArgument::Type(inner) = &args.args[0] {
                        return Ok(named("Option", vec![input_shape(inner)?]));
                    }
                }
            }
            return Err("unsupported Option input".into());
        }
    }
    owned_shape(ty)
}

pub fn native_methods(source: &str) -> Result<Methods, String> {
    let file = syn::parse_file(source).map_err(|error| error.to_string())?;
    let traits: Vec<_> = file
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Trait(item) if item.ident == "BrowserBackend" => Some(item),
            _ => None,
        })
        .collect();
    if traits.len() != 1 {
        return Err("expected exactly one BrowserBackend trait".into());
    }
    let mut methods = Methods::new();
    let mut declared_names = BTreeSet::new();
    for item in &traits[0].items {
        let TraitItem::Fn(method) = item else {
            return Err("unsupported BrowserBackend trait item".into());
        };
        let sig = &method.sig;
        let name = sig.ident.to_string();
        if !declared_names.insert(name.clone()) {
            return Err(format!("duplicate native method {name}"));
        }
        if name == "as_any" {
            continue;
        }
        if name == "last_effect_verification" && sig.asyncness.is_some() {
            return Err("effect getter must remain synchronous".into());
        }
        if sig.constness.is_some()
            || !matches!(&sig.safety, syn::Safety::Default)
            || sig.abi.is_some()
            || !sig.generics.params.is_empty()
            || sig.generics.where_clause.is_some()
            || sig.variadic.is_some()
        {
            return Err(format!("unsupported signature for {name}"));
        }
        let mut inputs = sig.inputs.iter();
        match inputs.next() {
            Some(FnArg::Receiver(receiver))
                if matches!(&receiver.kind, ReceiverKind::Reference(_, None, None))
                    && receiver.mutability.is_none() => {}
            _ => return Err(format!("expected &self for {name}")),
        }
        let mut parameters = Vec::new();
        let mut names = BTreeSet::new();
        for input in inputs {
            let FnArg::Typed(input) = input else {
                return Err(format!("unsupported receiver for {name}"));
            };
            let Pat::Ident(pattern) = &*input.pat else {
                return Err(format!("unsupported parameter pattern for {name}"));
            };
            if pattern.by_ref.is_some() || pattern.mutability.is_some() || pattern.subpat.is_some()
            {
                return Err(format!("unsupported parameter binding for {name}"));
            }
            let parameter = pattern.ident.to_string();
            if !names.insert(parameter.clone()) {
                return Err(format!("duplicate parameter in {name}: {parameter}"));
            }
            parameters.push((parameter, input_shape(&input.ty)?));
        }
        let ReturnType::Type(_, ty) = &sig.output else {
            return Err(format!("missing return for {name}"));
        };
        let output = owned_shape(ty)?;
        let success = if sig.asyncness.is_some() {
            match output {
                Shape::Named(result, args)
                    if result == "Result"
                        && args.len() == 2
                        && args[1] == named("BrowserError", vec![]) =>
                {
                    args[0].clone()
                }
                _ => return Err(format!("expected Result<T, BrowserError> for {name}")),
            }
        } else {
            if name != "last_effect_verification"
                || !parameters.is_empty()
                || output != named("Option", vec![named("EffectVerification", vec![])])
            {
                return Err(format!("unsupported synchronous method {name}"));
            }
            output
        };
        if methods
            .insert(
                name.clone(),
                Signature {
                    parameters,
                    success,
                },
            )
            .is_some()
        {
            return Err(format!("duplicate native method {name}"));
        }
    }
    if methods.is_empty() {
        return Err("empty BrowserBackend inventory".into());
    }
    Ok(methods)
}

struct Declaration(Vec<Row>);

impl Parse for Declaration {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut rows = Vec::new();
        while !input.is_empty() {
            let method: syn::Ident = input.parse()?;
            input.parse::<Token![=>]>()?;
            let variant: syn::Ident = input.parse()?;
            let fields;
            braced!(fields in input);
            let mut parameters = Vec::new();
            let mut names = BTreeSet::new();
            while !fields.is_empty() {
                let field: syn::Ident = fields.parse()?;
                fields.parse::<Token![:]>()?;
                let ty: Type = fields.parse()?;
                if !names.insert(field.to_string()) {
                    return Err(fields.error("duplicate operation field"));
                }
                parameters.push((
                    field.to_string(),
                    owned_shape(&ty).map_err(|error| fields.error(error))?,
                ));
                if fields.is_empty() {
                    break;
                }
                fields.parse::<Token![,]>()?;
            }
            input.parse::<Token![->]>()?;
            let ty: Type = input.parse()?;
            input.parse::<Token![;]>()?;
            rows.push(Row {
                method: method.to_string(),
                variant: variant.to_string(),
                signature: Signature {
                    parameters,
                    success: owned_shape(&ty).map_err(|error| input.error(error))?,
                },
            });
        }
        if rows.is_empty() {
            return Err(input.error("empty operation declaration"));
        }
        Ok(Self(rows))
    }
}

fn invocation(file: &syn::File) -> Result<&syn::ItemMacro, String> {
    let invocations: Vec<_> = file
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Macro(item) if item.mac.path.is_ident("declare_browser_operations") => Some(item),
            _ => None,
        })
        .collect();
    if invocations.len() != 1 {
        return Err("expected exactly one local declare_browser_operations invocation".into());
    }
    let item = invocations[0];
    if item.ident.is_some() || !matches!(item.mac.delimiter, syn::MacroDelimiter::Brace(_)) {
        return Err("unsupported operation invocation shape".into());
    }
    Ok(item)
}

fn rows_from_file(file: &syn::File) -> Result<Vec<Row>, String> {
    let rows = syn::parse2::<Declaration>(invocation(file)?.mac.tokens.clone())
        .map_err(|error| error.to_string())?
        .0;
    let mut methods = BTreeSet::new();
    let mut variants = BTreeSet::new();
    for row in &rows {
        if !methods.insert(row.method.clone()) {
            return Err(format!("duplicate operation method {}", row.method));
        }
        if !variants.insert(row.variant.clone()) {
            return Err(format!(
                "duplicate operation/reply/feature variant {}",
                row.variant
            ));
        }
    }
    Ok(rows)
}

pub fn operation_rows(source: &str) -> Result<Vec<Row>, String> {
    rows_from_file(&syn::parse_file(source).map_err(|error| error.to_string())?)
}

pub fn compare(native: &Methods, rows: &[Row]) -> Result<(), String> {
    if native.is_empty() || rows.is_empty() {
        return Err("empty inventory".into());
    }
    let mut methods = BTreeSet::new();
    let mut variants = BTreeSet::new();
    for row in rows {
        if !methods.insert(row.method.clone()) || !variants.insert(row.variant.clone()) {
            return Err("duplicate mapping".into());
        }
        let Some(signature) = native.get(&row.method) else {
            return Err(format!("extra mapping {}", row.method));
        };
        if signature != &row.signature {
            return Err(format!(
                "signature mismatch {}: native {signature:?}, API {:?}",
                row.method, row.signature
            ));
        }
    }
    let missing: Vec<_> = native
        .keys()
        .filter(|method| !methods.contains(*method))
        .collect();
    if !missing.is_empty() {
        return Err(format!("missing mapping {missing:?}"));
    }
    Ok(())
}

pub fn census() -> (Methods, Vec<Row>) {
    let native = native_methods(NATIVE_SOURCE).expect("parse actual native trait");
    let rows = operation_rows(OPERATIONS_SOURCE).expect("parse actual API invocation");
    compare(&native, &rows).expect("native methods and API rows must agree");
    (native, rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mutate token trees from the REAL invocation, never a hand-maintained row.
    fn mutate_rows(kind: &str) -> syn::File {
        let mut file = syn::parse_file(OPERATIONS_SOURCE).unwrap();
        let item = file
            .items
            .iter_mut()
            .find_map(|item| match item {
                Item::Macro(item) if item.mac.path.is_ident("declare_browser_operations") => {
                    Some(item)
                }
                _ => None,
            })
            .unwrap();
        let tokens: Vec<_> = item.mac.tokens.clone().into_iter().collect();
        // Top-level semicolon terminates a row; field/type groups are token trees.
        let first_end = tokens
            .iter()
            .position(|token| token.to_string() == ";")
            .unwrap()
            + 1;
        item.mac.tokens = match kind {
            "delete" => tokens[first_end..].iter().cloned().collect(),
            "duplicate" => tokens
                .iter()
                .chain(tokens[..first_end].iter())
                .cloned()
                .collect(),
            "empty" => Default::default(),
            _ => panic!("unknown mutation"),
        };
        file
    }

    #[test]
    fn browser_inventory_deleted_real_macro_row_is_missing_clean_bad_clean() {
        let native = native_methods(NATIVE_SOURCE).unwrap();
        let clean = operation_rows(OPERATIONS_SOURCE).unwrap();
        compare(&native, &clean).unwrap();
        let bad = rows_from_file(&mutate_rows("delete")).unwrap();
        assert_eq!(bad.len() + 1, clean.len());
        assert!(compare(&native, &bad)
            .unwrap_err()
            .contains("missing mapping"));
        compare(
            &native_methods(NATIVE_SOURCE).unwrap(),
            &operation_rows(OPERATIONS_SOURCE).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn browser_inventory_duplicate_real_macro_row_is_rejected_clean_bad_clean() {
        census();
        assert!(rows_from_file(&mutate_rows("duplicate"))
            .unwrap_err()
            .contains("duplicate"));
        census();
    }

    #[test]
    fn browser_inventory_empty_or_missing_roots_are_rejected_clean_bad_clean() {
        census();
        assert!(rows_from_file(&mutate_rows("empty"))
            .unwrap_err()
            .contains("empty"));
        assert!(operation_rows("").unwrap_err().contains("exactly one"));
        assert!(native_methods("trait BrowserBackend {}")
            .unwrap_err()
            .contains("empty"));
        assert!(native_methods("").unwrap_err().contains("exactly one"));
        assert!(compare(&Methods::new(), &[]).is_err());
        census();
    }

    #[test]
    fn browser_inventory_signature_drift_and_unsupported_syntax_are_rejected() {
        let (native, rows) = census();
        let mut reordered = rows.clone();
        let row = reordered
            .iter_mut()
            .find(|row| row.signature.parameters.len() > 1)
            .unwrap();
        row.signature.parameters.swap(0, 1);
        assert!(compare(&native, &reordered)
            .unwrap_err()
            .contains("signature mismatch"));
        let mut wrong_parameter = rows.clone();
        let row = wrong_parameter
            .iter_mut()
            .find(|row| !row.signature.parameters.is_empty())
            .unwrap();
        row.signature.parameters[0].1 = named("u64", vec![]);
        assert!(compare(&native, &wrong_parameter)
            .unwrap_err()
            .contains("signature mismatch"));
        let mut wrong_return = rows.clone();
        wrong_return[0].signature.success = Shape::Tuple(vec![]);
        assert!(compare(&native, &wrong_return)
            .unwrap_err()
            .contains("signature mismatch"));
        assert!(
            operation_rows("declare_browser_operations! { verb => Verb { x: &str } -> (); }")
                .is_err()
        );
        assert!(operation_rows(
            "declare_browser_operations! { verb => Verb {} -> (); other => Verb {} -> (); }"
        )
        .unwrap_err()
        .contains("duplicate"));
        assert!(operation_rows(
            "declare_browser_operations! { verb => Verb { x: String, x: String } -> (); }"
        )
        .unwrap_err()
        .contains("duplicate"));
        assert!(operation_rows("declare_browser_operations! { verb => Verb {} -> (); } declare_browser_operations! { other => Other {} -> (); }")
            .unwrap_err().contains("exactly one"));
        assert!(
            native_methods("trait BrowserBackend {} trait BrowserBackend {}")
                .unwrap_err()
                .contains("exactly one")
        );
        assert!(native_methods("trait BrowserBackend { async fn last_effect_verification(&self) -> Result<Option<EffectVerification>, BrowserError>; }")
            .unwrap_err().contains("synchronous"));
        assert!(native_methods(
            "trait BrowserBackend { async unsafe fn x(&self) -> Result<(), BrowserError>; }"
        )
        .unwrap_err()
        .contains("unsupported signature"));
        for receiver in [
            "&mut self",
            "self",
            "mut self",
            "self: &Self",
            "self: Arc<Self>",
            "&'static self",
        ] {
            let source = format!(
                "trait BrowserBackend {{ async fn x({receiver}) -> Result<(), BrowserError>; }}"
            );
            let error = native_methods(&source).unwrap_err();
            assert!(
                error.contains("expected &self for x"),
                "{receiver}: {error}"
            );
        }
        assert!(native_methods("trait BrowserBackend { async fn x(&self, value: &mut str) -> Result<(), BrowserError>; }").is_err());
        assert!(native_methods("trait BrowserBackend { async fn x(&self) -> Result<(), BrowserError>; async fn x(&self) -> Result<(), BrowserError>; }")
            .unwrap_err().contains("duplicate"));
        census();
    }
}
