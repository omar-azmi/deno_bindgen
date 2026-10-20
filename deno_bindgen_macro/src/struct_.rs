use proc_macro2::TokenStream as TokenStream2;
use syn::{GenericParam, ItemStruct};

use crate::util::Result;
use crate::util::{self};

pub fn handle(struct_: ItemStruct) -> Result<TokenStream2> {
  if struct_
    .generics
    .params
    .iter()
    .any(|param| !matches!(param, GenericParam::Lifetime(_)))
  {
    return Err(util::Error::Generics);
  }

  let ref ty_str @ _ = struct_.ident;
  let (impl_generics, ty_generics, where_clause) =
    struct_.generics.split_for_impl();
  Ok(quote::quote! {
    #struct_

    impl #impl_generics ::deno_bindgen::BindgenType for #ty_str #ty_generics #where_clause {
        fn type_name() -> &'static str {
            stringify!(#ty_str)
        }
    }
  })
}
