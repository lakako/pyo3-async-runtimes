use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::spanned::Spanned;

#[derive(Clone, Copy, PartialEq)]
enum RuntimeFlavor {
    CurrentThread,
    Threaded,
}

impl RuntimeFlavor {
    fn from_str(s: &str) -> Result<RuntimeFlavor, String> {
        match s {
            "current_thread" => Ok(RuntimeFlavor::CurrentThread),
            "multi_thread" => Ok(RuntimeFlavor::Threaded),
            "single_thread" => Err("The single threaded runtime flavor is called `current_thread`.".to_string()),
            "basic_scheduler" => Err("The `basic_scheduler` runtime flavor has been renamed to `current_thread`.".to_string()),
            "threaded_scheduler" => Err("The `threaded_scheduler` runtime flavor has been renamed to `multi_thread`.".to_string()),
            _ => Err(format!("No such runtime flavor `{}`. The runtime flavors are `current_thread` and `multi_thread`.", s)),
        }
    }
}

struct FinalConfig {
    flavor: RuntimeFlavor,
    worker_threads: Option<usize>,
}

struct Configuration {
    rt_multi_thread_available: bool,
    default_flavor: RuntimeFlavor,
    flavor: Option<RuntimeFlavor>,
    worker_threads: Option<(usize, Span)>,
}

impl Configuration {
    fn new(is_test: bool, rt_multi_thread: bool) -> Self {
        Configuration {
            rt_multi_thread_available: rt_multi_thread,
            default_flavor: match is_test {
                true => RuntimeFlavor::CurrentThread,
                false => RuntimeFlavor::Threaded,
            },
            flavor: None,
            worker_threads: None,
        }
    }

    fn set_flavor(&mut self, runtime: syn::Lit, span: Span) -> Result<(), syn::Error> {
        if self.flavor.is_some() {
            return Err(syn::Error::new(span, "`flavor` set multiple times."));
        }

        let runtime_str = parse_string(runtime, span, "flavor")?;
        let runtime =
            RuntimeFlavor::from_str(&runtime_str).map_err(|err| syn::Error::new(span, err))?;
        self.flavor = Some(runtime);
        Ok(())
    }

    fn set_worker_threads(
        &mut self,
        worker_threads: syn::Lit,
        span: Span,
    ) -> Result<(), syn::Error> {
        if self.worker_threads.is_some() {
            return Err(syn::Error::new(
                span,
                "`worker_threads` set multiple times.",
            ));
        }

        let worker_threads = parse_int(worker_threads, span, "worker_threads")?;
        if worker_threads == 0 {
            return Err(syn::Error::new(span, "`worker_threads` may not be 0."));
        }
        self.worker_threads = Some((worker_threads, span));
        Ok(())
    }

    fn build(&self) -> Result<FinalConfig, syn::Error> {
        let flavor = self.flavor.unwrap_or(self.default_flavor);
        use RuntimeFlavor::*;
        match (flavor, self.worker_threads) {
            (CurrentThread, Some((_, worker_threads_span))) => Err(syn::Error::new(
                worker_threads_span,
                "The `worker_threads` option requires the `multi_thread` runtime flavor.",
            )),
            (CurrentThread, None) => Ok(FinalConfig {
                flavor,
                worker_threads: None,
            }),
            (Threaded, worker_threads) if self.rt_multi_thread_available => Ok(FinalConfig {
                flavor,
                worker_threads: worker_threads.map(|(val, _span)| val),
            }),
            (Threaded, _) => {
                let msg = if self.flavor.is_none() {
                    "The default runtime flavor is `multi_thread`, but the `rt-multi-thread` feature is disabled."
                } else {
                    "The runtime flavor `multi_thread` requires the `rt-multi-thread` feature."
                };
                Err(syn::Error::new(Span::call_site(), msg))
            }
        }
    }
}

fn parse_int(int: syn::Lit, span: Span, field: &str) -> Result<usize, syn::Error> {
    match int {
        syn::Lit::Int(lit) => match lit.base10_parse::<usize>() {
            Ok(value) => Ok(value),
            Err(e) => Err(syn::Error::new(
                span,
                format!("Failed to parse {} as integer: {}", field, e),
            )),
        },
        _ => Err(syn::Error::new(
            span,
            format!("Failed to parse {} as integer.", field),
        )),
    }
}

fn parse_string(int: syn::Lit, span: Span, field: &str) -> Result<String, syn::Error> {
    match int {
        syn::Lit::Str(s) => Ok(s.value()),
        syn::Lit::Verbatim(s) => Ok(s.to_string()),
        _ => Err(syn::Error::new(
            span,
            format!("Failed to parse {} as string.", field),
        )),
    }
}

fn parse_knobs(
    input: syn::ItemFn,
    args: Vec<syn::Meta>,
    is_test: bool,
    rt_multi_thread: bool,
) -> Result<TokenStream, syn::Error> {
    let sig = &input.sig;
    let ret = &input.sig.output;
    let body = &input.block;
    let attrs = &input.attrs;
    let vis = input.vis;

    if sig.asyncness.is_none() {
        let msg = "the async keyword is missing from the function declaration";
        return Err(syn::Error::new_spanned(sig.fn_token, msg));
    }

    let macro_name = "pyo3_async_runtimes::tokio::main";
    let mut config = Configuration::new(is_test, rt_multi_thread);

    for arg in args {
        match arg {
            syn::Meta::NameValue(namevalue) => {
                let ident = namevalue.path.get_ident();
                if ident.is_none() {
                    let msg = "Must have specified ident";
                    return Err(syn::Error::new_spanned(namevalue, msg));
                }
                match ident.unwrap().to_string().to_lowercase().as_str() {
                    "worker_threads" => {
                        if let syn::Expr::Lit(expr_lit) = &namevalue.value {
                            config.set_worker_threads(expr_lit.lit.clone(), namevalue.span())?;
                        } else {
                            return Err(syn::Error::new_spanned(
                                &namevalue.value,
                                "Expected a literal value",
                            ));
                        }
                    }
                    "flavor" => {
                        if let syn::Expr::Lit(expr_lit) = &namevalue.value {
                            config.set_flavor(expr_lit.lit.clone(), namevalue.span())?;
                        } else {
                            return Err(syn::Error::new_spanned(
                                &namevalue.value,
                                "Expected a literal value",
                            ));
                        }
                    }
                    "core_threads" => {
                        let msg = "Attribute `core_threads` is renamed to `worker_threads`";
                        return Err(syn::Error::new_spanned(namevalue, msg));
                    }
                    name => {
                        let msg = format!("Unknown attribute {} is specified; expected one of: `flavor`, `worker_threads`", name);
                        return Err(syn::Error::new_spanned(namevalue, msg));
                    }
                }
            }
            syn::Meta::Path(path) => {
                let ident = path.get_ident();
                if ident.is_none() {
                    let msg = "Must have specified ident";
                    return Err(syn::Error::new_spanned(path, msg));
                }
                let name = ident.unwrap().to_string().to_lowercase();
                let msg = match name.as_str() {
                    "threaded_scheduler" | "multi_thread" => {
                        format!(
                            "Set the runtime flavor with #[{}(flavor = \"multi_thread\")].",
                            macro_name
                        )
                    }
                    "basic_scheduler" | "current_thread" | "single_threaded" => {
                        format!(
                            "Set the runtime flavor with #[{}(flavor = \"current_thread\")].",
                            macro_name
                        )
                    }
                    "flavor" | "worker_threads" => {
                        format!("The `{}` attribute requires an argument.", name)
                    }
                    name => {
                        format!("Unknown attribute {} is specified; expected one of: `flavor`, `worker_threads`", name)
                    }
                };
                return Err(syn::Error::new_spanned(path, msg));
            }
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "Unknown attribute inside the macro",
                ));
            }
        }
    }

    let config = config.build()?;

    let builder = match config.flavor {
        RuntimeFlavor::CurrentThread => quote! {
            pyo3_async_runtimes::tokio::re_exports::runtime::Builder::new_current_thread()
        },
        RuntimeFlavor::Threaded => quote! {
            pyo3_async_runtimes::tokio::re_exports::runtime::Builder::new_multi_thread()
        },
    };

    let mut builder_init = quote! {
        builder.enable_all();
    };
    if let Some(v) = config.worker_threads {
        builder_init = quote! {
            builder.worker_threads(#v);
            #builder_init;
        };
    }

    let rt_init = match config.flavor {
        RuntimeFlavor::CurrentThread => quote! {
            std::thread::spawn(|| pyo3_async_runtimes::tokio::get_runtime().block_on(
                pyo3_async_runtimes::tokio::re_exports::pending::<()>()
            ));
        },
        _ => quote! {},
    };

    let result = quote! {
        #(#attrs)*
        #vis fn main() {
            async fn main() #ret {
                #body
            }

            pyo3::prepare_freethreaded_python();

            let mut builder = #builder;
            #builder_init;

            pyo3_async_runtimes::tokio::init(builder);

            #rt_init

            pyo3::Python::with_gil(|py| {
                pyo3_async_runtimes::tokio::run(py, main())
                    .map_err(|e| {
                        e.print_and_set_sys_last_vars(py);
                    })
                    .unwrap();
            });
        }
    };

    Ok(result.into())
}

#[cfg(not(test))] // Work around for rust-lang/rust#62127
pub(crate) fn main(args: TokenStream, item: TokenStream, rt_multi_thread: bool) -> TokenStream {
    let input = syn::parse_macro_input!(item as syn::ItemFn);
    let args = syn::parse_macro_input!(args with syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated);
    let args: Vec<syn::Meta> = args.into_iter().collect();

    if input.sig.ident == "main" && !input.sig.inputs.is_empty() {
        let msg = "the main function cannot accept arguments";
        return syn::Error::new_spanned(&input.sig.ident, msg)
            .to_compile_error()
            .into();
    }

    parse_knobs(input, args, false, rt_multi_thread).unwrap_or_else(|e| e.to_compile_error().into())
}
