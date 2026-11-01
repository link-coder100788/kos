use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Error, Expr, Ident, ItemFn, LitInt, LitStr, Path, Token, parse::Parse, parse_macro_input, parse_quote, spanned::Spanned};

enum InitcallLevel {
    Early,
    Middle,
    Late,
}

impl InitcallLevel {
    fn parse(from: &str) -> Option<Self> {
        match from {
            "early" => Some(Self::Early),
            "middle" => Some(Self::Middle),
            "late" => Some(Self::Late),
            _ => None,
        }
    }
}

impl From<&str> for InitcallLevel {
    fn from(s: &str) -> Self {
        Self::parse(s).unwrap_or(Self::Early)
    }
}

impl ToString for InitcallLevel {
    fn to_string(&self) -> String {
        match self {
            InitcallLevel::Early => "early",
            InitcallLevel::Middle => "middle",
            InitcallLevel::Late => "late",
        }.to_string()
    }
}

struct InitcallArgs {
    level: InitcallLevel,
    priority: u8,
}

impl Parse for InitcallArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut level = None;
        let mut priority = None;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "level" => {
                    let path: Path = input.parse()?;
                    let segment = path.segments.last().ok_or_else(|| Error::new(path.span(), "expected InitLevel::<variant>"))?;
                    level = Some(match InitcallLevel::parse(segment.ident.to_string().as_str()) {
                        Some(l) => l,
                        None => return Err(Error::new(segment.ident.span(), "expected <level> (early, middle, late)")),
                    });
                }
                "priority" => {
                    let lit: LitInt = input.parse()?;
                    let value = lit.base10_parse::<u8>()?;
                    if !(1 as u8..=10 as u8).contains(&value) {
                        return Err(Error::new(lit.span(), "expected <priority> (1..=10)"));
                    }
                    priority = Some(value);
                }
                _ => return Err(Error::new(key.span(), "expected <key> (level, priority)")),
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(InitcallArgs { level: level.ok_or_else(|| { Error::new(proc_macro2::Span::call_site(), "missing level") })?, priority: priority.ok_or_else(|| { Error::new(proc_macro2::Span::call_site(), "missing priority") })? })
    }
}

#[proc_macro_attribute]
pub fn initcall(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as InitcallArgs);
    let func = parse_macro_input!(item as ItemFn);
    let name = &func.sig.ident;
    let section = LitStr::new(format!(".initcall.{}.{:02}", args.level.to_string(), args.priority).as_str(), proc_macro2::Span::call_site());
    let static_name = format_ident!("__INITCALL_{}", name);
    TokenStream::from(quote! {
        #func

        #[used]
        #[unsafe(link_section = #section)]
        #[allow(non_upper_case_globals)]
        static #static_name: crate::init::Initcall = crate::init::Initcall { name: stringify!(#name), func: #name };
    })
}

#[proc_macro_attribute]
pub fn trace(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut func = parse_macro_input!(item as ItemFn);
    let name = &func.sig.ident;
    let entrymsg = LitStr::new(format!("[Trace: Entry]: {}", name).as_str(), proc_macro2::Span::call_site());
    let exitmsg = LitStr::new(format!("[Trace: Exit]: {}", name).as_str(), proc_macro2::Span::call_site());
    func.block.stmts.insert(0, parse_quote!({ println!(#entrymsg); }));
    func.block.stmts.push(parse_quote!({ println!(#exitmsg); }));
    TokenStream::from(quote! {
        #func
    })
}

struct ConstAssert {
    expr: Expr,
    message: Option<LitStr>
}

impl Parse for ConstAssert {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let expr = input.parse::<Expr>()?;
        let message = if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            Some(input.parse::<LitStr>()?)
        } else {
            None
        };
        Ok(Self { expr, message })
    }
}

#[proc_macro]
pub fn const_assert(input: TokenStream) -> TokenStream {
    let ConstAssert { expr, message } = parse_macro_input!(input as ConstAssert);
    let expanded = if let Some(msg) = message {
        quote! {
            const _: () = {
                assert!(#expr, #msg);
            };
        }
    } else {
        quote! {
            const _: () = {
                assert!(#expr);
            };
        }
    };
    TokenStream::from(expanded)
}

#[proc_macro]
pub fn const_assert_dbg(input: TokenStream) -> TokenStream {
    let ConstAssert { expr, message } = parse_macro_input!(input as ConstAssert);
    let expanded = if let Some(msg) = message {
        quote! {
            const _: () = {
                debug_assert!(#expr, #msg);
            };
        }
    } else {
        quote! {
            const _: () = {
                debug_assert!(#expr);
            };
        }
    };
    TokenStream::from(expanded)
}

#[proc_macro_attribute]
pub fn syscall(attr: TokenStream, item: TokenStream) -> TokenStream {
    let number = parse_macro_input!(attr as LitInt);
    let number = match number.base10_parse::<u64>() {
        Ok(n) => n,
        Err(e) => return e.to_compile_error().into(),
    };
    let input = parse_macro_input!(item as ItemFn);
    let fn_name = &input.sig.ident;
    let static_name = format_ident!("__SYSCALL_{}", fn_name);
    TokenStream::from(quote! {
        #input

        #[used]
        #[unsafe(link_section = ".syscalls")]
        #[allow(non_upper_case_globals)]
        static #static_name: crate::syscall::SyscallEntry = crate::syscall::SyscallEntry { number: #number, name: stringify!(#fn_name), handler: #fn_name };
    })
}

struct Idents {
    idents: Vec<Ident>,
}

impl Parse for Idents {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        Ok(Self { idents: input.parse_terminated(Ident::parse, Token![,])?.into_iter().collect() })
    }
}

#[proc_macro]
pub fn const_unique(input: TokenStream) -> TokenStream {
    let idents: Idents = parse_macro_input!(input as Idents);
    let mut asserts = Vec::new();
    for i in 0..idents.idents.len() {
        for j in (i + 1)..idents.idents.len() {
            let a = &idents.idents[i];
            let b = &idents.idents[j];
            let msg = format!("{a} and {b} must have unique values");
            asserts.push(quote! {
                assert!(#a != #b, #msg);
            });
        }
    }
    TokenStream::from(quote! {
        const _: () = {
            #(#asserts)*
        };
    })
}
