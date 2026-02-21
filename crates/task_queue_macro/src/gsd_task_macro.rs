use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Error, Fields, parse_macro_input};

pub(crate) fn gsd_task_macro(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);

    let enum_data = match &input.data {
        Data::Enum(data) => data,
        _ => {
            return Error::new_spanned(&input, "GsdTask can only be derived on enums")
                .to_compile_error()
                .into();
        }
    };

    let enum_name = &input.ident;
    let in_progress_enum_name = format_ident!("{}InProgress", enum_name);

    let mut variant_names = Vec::new();
    let mut variant_types = Vec::new();

    for variant in &enum_data.variants {
        let variant_name = &variant.ident;

        let inner_type = match &variant.fields {
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => &fields.unnamed[0].ty,
            _ => {
                return Error::new_spanned(
                    variant,
                    "GsdTask variants must be tuple variants with exactly one field",
                )
                .to_compile_error()
                .into();
            }
        };

        variant_names.push(variant_name.clone());
        variant_types.push(inner_type.clone());
    }

    let in_progress_variants = variant_names
        .iter()
        .zip(variant_types.iter())
        .map(|(name, ty)| {
            let doc = format!("In-progress state for [`{name}`].");
            quote! {
                #[doc = #doc]
                #name(<#ty as QueueItem<Ctx>>::InProgress)
            }
        });

    let from_impls = variant_names
        .iter()
        .zip(variant_types.iter())
        .map(|(name, ty)| {
            quote! {
                impl From<#ty> for #enum_name {
                    fn from(item: #ty) -> Self {
                        #enum_name::#name(item)
                    }
                }
            }
        });

    let start_arms = variant_names.iter().map(|name| {
        quote! {
            #enum_name::#name(item) => {
                let (ip, cmd) = item.start(ctx);
                (#in_progress_enum_name::#name(ip), cmd)
            }
        }
    });

    let cleanup_arms = variant_names
        .iter()
        .zip(variant_types.iter())
        .map(|(name, ty)| {
            quote! {
                #in_progress_enum_name::#name(ip) => {
                    <#ty as QueueItem<Ctx>>::cleanup(ip, result, ctx).into_tasks()
                }
            }
        });

    let in_progress_doc = format!("In-progress state for [`{enum_name}`].");
    let output = quote! {
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

            fn cleanup(
                in_progress: Self::InProgress,
                result: Result<Self::Response, ::serde_json::Error>,
                ctx: &mut Ctx,
            ) -> Self::NextTasks {
                match in_progress {
                    #(#cleanup_arms),*
                }
            }
        }
    };

    output.into()
}
