use proc_macro::TokenStream;
use proc_macro2::Span;
use proc_macro_crate::crate_name;
// use proc_macro2::TokenStream;
use quote::{quote, ToTokens};
use syn::{parse_macro_input, ItemFn, Ident, parse::Parse, punctuated::Punctuated, Token};


struct CreateEntryArguments {
    no_console: bool
}

impl Parse for CreateEntryArguments {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let argss = Punctuated::<Ident, Token![,]>::parse_terminated(input)?;
        for arg in argss {
            if arg == "no_console" {
                return Ok(CreateEntryArguments {
                    no_console: true
                });
            }
        }
        return Ok(CreateEntryArguments {
            no_console: false
        });
    }
}


/// This macro allows you to define a function which will be called upon dll injection
/// ## Notes
/// On windows, this will automatically allocate a console, if you don't wan't do do that, use the `no_console` attribute
#[proc_macro_attribute]
pub fn create_entry(attr:TokenStream, item:TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemFn);
    let arg = parse_macro_input!(attr as CreateEntryArguments);
    let input_name = input.sig.ident.clone();


    let curr_crate = match crate_name("poggers").expect("poggers-derive to be found") {
        proc_macro_crate::FoundCrate::Itself => quote!(crate),
        proc_macro_crate::FoundCrate::Name(x) => {
            let i = Ident::new(&x, Span::call_site());
            quote!(#i)
        },
    };

    let ret = input.sig.output.clone();

    let handle_ret = match ret {
        syn::ReturnType::Default => quote!(),
        syn::ReturnType::Type(_, ty) => {
            if ty.to_token_stream().to_string().contains("Result") {
                quote!{
                    match r {
                        Ok(_) => (),
                        Err(e) => {
                            println!(concat!(stringify!{#input_name}," has errored: {:?}"), e);
                        }
                    }
                }
            } else {
                quote!()
            }
        },
    };

    let alloc_console = if arg.no_console {quote!{}} else {quote!{
        unsafe {
            #curr_crate::exports::AllocConsole();
        };
    }};
    let free_console = if arg.no_console {quote!{}} else {quote!{
        unsafe {
            #curr_crate::exports::FreeConsole();
        };
    }};

    let cross_platform = quote!{
        use ::std::panic;

        match panic::catch_unwind(||#input_name()) {
            Err(e) => {
                println!("`{}` has panicked: {:#?}",stringify!{#input_name}, e);
            }
            Ok(r) => {#handle_ret},
        };
    };
    #[cfg(target_os = "windows")]
    let generated = quote!{
        #[no_mangle]
        extern "system" fn DllMain(
            h_module : #curr_crate::exports::HINSTANCE,
            reason : u32,
            _: *const ::std::ffi::c_void
        ) -> #curr_crate::exports::BOOL {
            match reason {
                #curr_crate::exports::DLL_PROCESS_ATTACH => {
                    std::thread::spawn(|| {
                        #alloc_console
                        #cross_platform

                        #free_console
                    });
                    (true).into()
                }
                _ => (false).into()
            }
        }
    };
    #[cfg(not(target_os = "windows"))]
    let generated = quote!{
        #[#curr_crate::exports::ctor]
        fn lib_init() {
            std::thread::spawn(|| {

                #cross_platform

            });
        }
    };


    TokenStream::from(quote!{
        #input

        #generated
    })
}