//! `include-sql` is a macro for *using* SQL from Rust. When using `include-sql` you
//! create raw `.sql` files that contain your queries and then execute them from Rust.
//!
//! # Example
//!
//! SQL file `src/crew.sql`
//! ```sql
//! -- name: select_ship_crew
//! -- Selects ship crew (sailors of a given ship)
//! SELECT id, name, rank
//!   FROM sailors
//!  WHERE ship_id = :ship
//! ```
//!
//! To execute this query then in Rust (let's say the database is Postgres)...
//! ```rust,no_run
//! use postgres::{ Connection, Result };
//! use postgres::types::ToSql;
//! use include_sql::include_sql;
//!
//! include_sql!("src/crew.sql","$");
//!
//! fn print_ship_crew(conn: &Connection, ship_id: i32) -> Result<()> {
//!     let rows = conn.query(SELECT_SHIP_CREW, using_select_ship_crew_args! {
//!         ship: &ship_id
//!     })?;
//!
//!     println!("Ship crew:");
//!     for row in &rows {
//!         let id : i32     = row.get(0);
//!         let name: String = row.get(1);
//!         let rank: String = row.get(2);
//!         println!(" - {} {}, ID: {}", rank, name, id);
//!     }
//!     Ok(())
//! }
//! ```
//!
//! # Features
//!
//! The basis of using SQL employed by `include-sql` was inspired by [Yesql](https://github.com/krisajenkins/yesql).
//! However `include-sql` is not a Yesql implemented in Rust as there is one key difference - `include-sql`
//! *assists* in using externally defined SQL, but it offloads the actual work to the database interface. Unlike Yesql
//! it does not generate functions that abstract database access.
//!

extern crate proc_macro;

use proc_macro::TokenStream;
use proc_macro2::Span;
use syn::{parse_macro_input, Token, Lit, LitStr, Ident, Expr, Error};
use syn::parse::{Parse, ParseStream, Result};
use syn::spanned::Spanned;
use quote::quote;

mod sql;

/// Includes SQL from the provided file.
///
/// This macro needs 2 arguments:
/// - Path to the SQL file. The path should be defined relative to the package root.
/// - Prefix that will the database interface uses to tag positional SQL parameters.
///   For example, it would be `"?"` for SQLite, `"$"` for Postgresql or `":"` for
///   Oracle.
///
/// There is an additional requirement. The code generated by the `include-sql` assumes that
/// the database interface has defined and implemented some trait to convert argument values
/// into a format suitable for sending to the database. The generated code expects that that
/// trait is called `ToSql` and thus this database specific trait must be imported by a module
/// as `ToSql`.
///
/// For example, with [rusqlite](https://github.com/jgallagher/rusqlite) one need to:
/// ```rust,no_run
/// use rusqlite::types::ToSql;
/// ```
/// With [rust-postgres](https://github.com/sfackler/rust-postgres):
/// ```rust,no_run
/// use postgres::types::ToSql;
/// ```
/// With [rust-oracle](https://github.com/kubo/rust-oracle):
/// ```rust,no_run
/// use oracle::ToSql;
/// ```
/// And with [oci_rs](https://github.com/huwwynnjones/oci_rs):
/// ```rust,no_run
/// use oci_rs::types::ToSqlValue as ToSql;
/// ```
///
/// For each of the statements found in the SQL file `include-sql` will generate:
/// - `&str` constant with the text of the preprocessed SQL - named parameters will be replaced
///   by numbered positional ones
/// - `struct` that will be used to convert query arguments from a named into a positional form
/// - a macro to transparently convert the argument struct into an argument slice when the struct
///   cannot be used directly
///
/// # Examples
///
/// Execution of queries with a dynamic `IN (:list)` component:
///
/// ```sql
/// -- name: select_ship_crew_by_rank
/// -- Selects sailors of a given ship that also have
/// -- specific ranks
/// SELECT id, name, rank
///   FROM sailors
///  WHERE ship_id = :ship
///    AND rank IN (:ranks)
/// ```
///
/// Query execution in Oracle:
///
/// ```rust,no_run
///   println!("Officers:");
///
///   let (sql, args) = SelectShipCrewByRank {
///       ship:  &ship_id,
///       ranks: &[ &"captain" as &ToSql, &"midshipman" ]
///   }.into_sql_with_args();
///
///   let rows = conn.query(&sql, &args)?;
///   for row in &rows {
///       let row = row?;
///
///       let id : i32     = row.get(0)?;
///       let name: String = row.get(1)?;
///       let rank: String = row.get(2)?;
///
///       println!(" - {} {}, ID: {}", rank, name, id);
///   }
/// ```
#[proc_macro]
pub fn include_sql(input: TokenStream) -> TokenStream {
    let IncludeSql { statements, param_prefix } = parse_macro_input!(input as IncludeSql);
    let mut code = Vec::new();

    for stmt in statements {
        let sql::Stmt { name, const_name, text, params } = stmt;
        code.push(quote! {
            const #const_name : &str = #text;
        });
        if let Some( params ) = params {
            if params.lst_params.is_empty() {
                add_pos_params(&params, &name, &mut code);
            } else {
                add_lst_params(&params, &param_prefix, &const_name, &mut code);
            }
        }
    }
    let code = quote! {
        #( #code )*
    };
    TokenStream::from(code)
}

struct IncludeSql {
    statements: Vec<sql::Stmt>,
    param_prefix: String
}

impl Parse for IncludeSql {
    fn parse(input: ParseStream) -> Result<Self> {
        let path: Expr = input.parse()?;
        input.parse::<Token![,]>()?;
        let param_prefix: Expr = input.parse()?;

        let path = to_litstr(path, "SQL file path")?;
        let path = path.value();
        let param_prefix = to_litstr(param_prefix, "parameter prefix")?;
        let param_prefix = param_prefix.value();
        match sql::parse_sql_file(&path, &param_prefix) {
            Ok(statements) => {
                Ok( IncludeSql { statements, param_prefix } )
            }
            Err(err) => {
                Err(Error::new(path.span(), format!("{}", err)))
            }
        }
    }
}

fn to_litstr(expr: Expr, kind: &str) -> Result<LitStr> {
    let span = expr.span();
    if let Expr::Lit( lit_expr ) = expr {
        if let Lit::Str( lit ) = lit_expr.lit {
            return Ok(lit);
        }
    }
    Err(Error::new(span, format!("{} must be a literal string", kind)))
}

macro_rules! len {
    ($s:expr) => {
        $s.len()
    };
    ($s:expr, $($t:expr),+) => {
        $s.len() + len!($($t),+)
    };
}

macro_rules! ident {
    ($s:expr) => {
        Ident::new($s, Span::call_site())
    };
    ($($s:expr),+) => {{
        let cap = len!($($s),+);
        let mut name = String::with_capacity(cap);
        $(
            name.push_str($s);
        )+
        ident!(&name)
    }};
}

fn add_pos_params(params: &sql::StmtParams, stmt_name: &str, code: &mut Vec<proc_macro2::TokenStream>) {
    let sql::StmtParams { struct_name, pos_params, lst_params: _ } = params;
    code.push(quote! {
        struct #struct_name<'a> {
            #( #pos_params : &'a dyn ToSql ),*
        }
    });
    let using_args_macro = ident!("using_", stmt_name, "_args");
    let args_macro = ident!(stmt_name, "_args");
    code.push(quote! {
        include_sql_helper::def_args!($ => #using_args_macro : #struct_name = #( #pos_params ),*);
        include_sql_helper::def_args!($ => #args_macro : #struct_name = #( #pos_params ),*);
    });
    let iter = ident!(&struct_name.to_string(), "ArgsIter");
    code.push(quote! {
        pub(crate) struct #iter<'a> {
            item: #struct_name<'a>,
            index: usize
        }
    });
    code.push(quote! {
        impl<'a> std::iter::IntoIterator for #struct_name<'a> {
            type Item = &'a dyn ToSql;
            type IntoIter = #iter<'a>;

            fn into_iter(self) -> Self::IntoIter {
                #iter { item: self, index: 0 }
            }
        }
    });
    let param_nums = 0..pos_params.len();
    let fn_next = quote! {
        fn next(&mut self) -> std::option::Option<Self::Item> {
            let next = match self.index {
                #( #param_nums => Some( self.item.#pos_params ), )*
                _ => None,
            };
            self.index += 1;
            next
        }
    };
    code.push(quote! {
        impl<'a> std::iter::Iterator for #iter<'a> {
            type Item = &'a dyn ToSql;
            #fn_next
        }
    });
}

fn add_lst_params(params: &sql::StmtParams, param_prefix: &str, sql_text_const: &Ident, code: &mut Vec<proc_macro2::TokenStream>) {
    let sql::StmtParams { struct_name, pos_params, lst_params } = params;

    struct ExtLstParam<'a> {
        param: &'a sql::LstParam,
        usage: ParamUsage
    }

    enum ParamUsage {
        Unique,
        HasDups,
        IsADup
    }

    let mut ext_lst_params = Vec::new();
    let mut lst_fields = Vec::new();
    let mut iter = lst_params.iter();
    if let Some( param ) = iter.next() {
        ext_lst_params.push(ExtLstParam { param, usage: ParamUsage::Unique });
        lst_fields.push(&param.name);
        for param in iter {
            if let Some( idx ) = ext_lst_params.iter().position(|ext| ext.param.name == param.name) {
                let ext = &mut ext_lst_params[idx];
                ext.usage = ParamUsage::HasDups;
                ext_lst_params.push(ExtLstParam { param, usage: ParamUsage::IsADup });
            } else {
                lst_fields.push(&param.name);
                ext_lst_params.push(ExtLstParam { param, usage: ParamUsage::Unique });
            }
        }
    }

    let mut push_lst_args_code = Vec::new();
    let mut from = 0;
    for ext in ext_lst_params {
        let param_name = &ext.param.name;
        let text_end = ext.param.position;
        push_lst_args_code.push(quote! {
            sql.push_str(&#sql_text_const[#from..#text_end]);
        });
        if let ParamUsage::HasDups = ext.usage {
            push_lst_args_code.push(quote! {
                let start = sql.len();
            });
        }
        match ext.usage {
            ParamUsage::Unique | ParamUsage::HasDups => {
                push_lst_args_code.push(quote! {
                    include_sql_helper::push(self.#param_name, #param_prefix, &mut sql, &mut args);
                });
            }
            ParamUsage::IsADup => {
            let param_list = ident!(&param_name.to_string(), "_list");
                push_lst_args_code.push(quote! {
                    sql.push_str(&#param_list);
                });
            }
        }
        if let ParamUsage::HasDups = ext.usage {
            let param_list = ident!(&param_name.to_string(), "_list");
            push_lst_args_code.push(quote! {
                let #param_list = sql[start .. sql.len()].to_string();
            });
        }
        from = text_end;
    }
    push_lst_args_code.push(quote! {
        sql.push_str(&#sql_text_const[#from..]);
    });

    code.push(quote! {
        struct #struct_name<'a> {
            #( #pos_params : &'a dyn ToSql, )*
            #( #lst_fields : &'a[&'a dyn ToSql] ),*
        }
    });
    code.push(quote! {
        impl<'a> #struct_name<'a>{
            fn into_sql_with_args(self) -> (String, Vec<&'a dyn ToSql>) {
                let mut args = Vec::new();
                #( args.push(self.#pos_params); )*
                let mut sql = String::with_capacity(#sql_text_const.len() + 16);
                #( #push_lst_args_code )*
                (sql, args)
            }
        }
    });
}
