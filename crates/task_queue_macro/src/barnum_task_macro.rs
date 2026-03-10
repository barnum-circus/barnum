//! Implementation of the `BarnumTask` derive macro.

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Error, Fields, Ident, Type, parse_macro_input};

/// Parsed variant info from the input enum.
struct VariantInfo {
    name: Ident,
    inner_type: Type,
}

/// Parse enum variants, returning an error if any variant is malformed.
fn parse_variants(data: &syn::DataEnum) -> Result<Vec<VariantInfo>, TokenStream> {
    let mut variants = Vec::new();

    for variant in &data.variants {
        let Fields::Unnamed(fields) = &variant.fields else {
            return Err(Error::new_spanned(
                variant,
                "BarnumTask variants must be tuple variants with exactly one field",
            )
            .to_compile_error()
            .into());
        };

        if fields.unnamed.len() != 1 {
            return Err(Error::new_spanned(
                variant,
                "BarnumTask variants must have exactly one field",
            )
            .to_compile_error()
            .into());
        }

        variants.push(VariantInfo {
            name: variant.ident.clone(),
            inner_type: fields.unnamed[0].ty.clone(),
        });
    }

    Ok(variants)
}

pub fn barnum_task_macro(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);

    let Data::Enum(enum_data) = &input.data else {
        return Error::new_spanned(&input, "BarnumTask can only be derived on enums")
            .to_compile_error()
            .into();
    };

    let variants = match parse_variants(enum_data) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let enum_name = &input.ident;
    let in_progress_enum_name = format_ident!("{enum_name}InProgress");

    let variant_types: Vec<_> = variants.iter().map(|v| &v.inner_type).collect();

    let in_progress_variants = variants.iter().map(|v| {
        let name = &v.name;
        let ty = &v.inner_type;
        let doc = format!("In-progress state for [`{name}`].");
        quote! {
            #[doc = #doc]
            #name(<#ty as QueueItem<Ctx>>::InProgress)
        }
    });

    let from_impls = variants.iter().map(|v| {
        let name = &v.name;
        let ty = &v.inner_type;
        quote! {
            impl From<#ty> for #enum_name {
                fn from(item: #ty) -> Self {
                    #enum_name::#name(item)
                }
            }
        }
    });

    let start_arms = variants.iter().map(|v| {
        let name = &v.name;
        quote! {
            #enum_name::#name(item) => {
                let (ip, cmd) = item.start(ctx);
                (#in_progress_enum_name::#name(ip), cmd)
            }
        }
    });

    let process_arms = variants.iter().map(|v| {
        let name = &v.name;
        let ty = &v.inner_type;
        quote! {
            #in_progress_enum_name::#name(ip) => {
                <#ty as QueueItem<Ctx>>::process(ip, result, ctx).into_tasks()
            }
        }
    });

    let in_progress_doc = format!("In-progress state for [`{enum_name}`].");

    quote! {
        #[doc = #in_progress_doc]
        pub enum #in_progress_enum_name<Ctx>
        where
            #(#variant_types: QueueItem<Ctx>,)*
        {
            #(#in_progress_variants),*
        }

        #(#from_impls)*

        impl<Ctx> QueueItem<Ctx> for #enum_name
        where
            #(#variant_types: QueueItem<Ctx, Response = ::serde_json::Value>,)*
            #(<#variant_types as QueueItem<Ctx>>::NextTasks: IntoTasks<#enum_name>,)*
        {
            type InProgress = #in_progress_enum_name<Ctx>;
            type Response = ::serde_json::Value;
            type NextTasks = Vec<#enum_name>;

            fn start(self, ctx: &mut Ctx) -> (Self::InProgress, ::std::process::Command) {
                match self {
                    #(#start_arms),*
                }
            }

            fn process(
                in_progress: Self::InProgress,
                result: Result<Self::Response, ::serde_json::Error>,
                ctx: &mut Ctx,
            ) -> Self::NextTasks {
                match in_progress {
                    #(#process_arms),*
                }
            }
        }
    }
    .into()
}
