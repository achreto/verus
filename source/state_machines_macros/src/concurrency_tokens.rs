#![allow(unused_imports)]

use crate::parse_token_stream::{MaybeSM, SMAndFuncs};
use crate::weakest::{get_safety_conditions, to_weakest};
use proc_macro2::Span;
use proc_macro2::TokenStream;
use quote::{quote, quote_spanned, ToTokens};
use smir::ast::{
    Arg, Extras, Field, Invariant, Lemma, LemmaPurpose, ShardableType, Transition, TransitionKind,
    TransitionStmt, SM,
};
use std::collections::HashSet;
use std::collections::HashMap;
use syn::buffer::Cursor;
use syn::parse::Error;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::token::Colon2;
use syn::visit_mut::VisitMut;
use syn::{
    braced, AttrStyle, Attribute, Expr, ExprField, ExprPath, FieldsNamed, FnArg, Ident,
    ImplItemMethod, Member, Meta, MetaList, NestedMeta, Path, PathArguments, PathSegment, Type,
};
use std::ops::Index;

fn inst_type_name(sm_name: &Ident) -> Ident {
    let name = sm_name.to_string() + "_Instance";
    Ident::new(&name, sm_name.span())
}

fn field_token_type_name(sm_name: &Ident, field: &Field<Ident, Type>) -> Ident {
    let name = sm_name.to_string() + "_" + &field.ident.to_string();
    Ident::new(&name, field.ident.span())
}

fn field_token_field_name(field: &Field<Ident, Type>) -> Ident {
    field.ident.clone()
}

fn field_token_field_type(field: &Field<Ident, Type>) -> Type {
    match &field.stype {
        ShardableType::Variable(ty) => ty.clone(),
    }
}

fn exchange_name(sm_name: &Ident, tr: &Transition<Span, Ident, Expr, Type>) -> Ident {
    let name = sm_name.to_string() + "_" + &tr.name.to_string();
    Ident::new(&name, tr.name.span())
}

fn transition_arg_name(field: &Field<Ident, Type>) -> Ident {
    let name = "t_input_".to_string() + &field.ident.to_string();
    Ident::new(&name, field.ident.span())
}

fn instance_struct_stream(sm: &SM<Span, Ident, ImplItemMethod, Expr, Type>) -> TokenStream {
    let insttype = inst_type_name(&sm.name);
    return quote! {
        #[spec]
        #[allow(non_camel_case_types)]
        pub struct #insttype {
            #[spec] pub id: ::builtin::int,
        }
    };
}

fn token_struct_stream(sm_name: &Ident, field: &Field<Ident, Type>) -> TokenStream {
    let tokenname = field_token_type_name(sm_name, field);
    let fieldname = field_token_field_name(field);
    let fieldtype = field_token_field_type(field);
    let insttype = inst_type_name(sm_name);

    return quote! {
        #[proof]
        #[verifier(unforgeable)]
        #[allow(non_camel_case_types)]
        pub struct #tokenname {
            #[spec] pub instance: #insttype,
            #[spec] pub #fieldname: #fieldtype,
        }
    };
}

pub fn output_token_types_and_fns(
    token_stream: &mut TokenStream,
    sm: &SM<Span, Ident, ImplItemMethod, Expr, Type>,
) -> syn::parse::Result<()> {
    token_stream.extend(instance_struct_stream(sm));
    for field in &sm.fields {
        token_stream.extend(token_struct_stream(&sm.name, field));
    }
    for tr in &sm.transitions {
        token_stream.extend(exchange_stream(&sm, tr)?);
    }
    Ok(())
}

struct Ctxt {
    fields_read: HashSet<Ident>,
    fields_written: HashSet<Ident>,
    requires: Vec<Expr>,
    ensures: Vec<Expr>,
    ident_to_field: HashMap<Ident, Field<Ident, Type>>,
}

impl Ctxt {
    pub fn get_field_by_ident(&self, span: Span, ident: &Ident) -> syn::parse::Result<Field<Ident, Type>> {
        match self.ident_to_field.get(ident) {
            Some(f) => Ok(f.clone()),
            None => Err(Error::new(span, "in a concurrent transition, any field access but be a state field")),
        }
    }

    pub fn mark_field_as_read(&mut self, field: &Field<Ident, Type>) {
        self.fields_read.insert(field.ident.clone());
    }
}

pub fn exchange_stream(
    sm: &SM<Span, Ident, ImplItemMethod, Expr, Type>,
    tr: &Transition<Span, Ident, Expr, Type>,
) -> syn::parse::Result<TokenStream> {
    let mut ident_to_field = HashMap::new();
    for field in &sm.fields {
        ident_to_field.insert(field.ident.clone(), field.clone());
    }

    let mut ctxt = Ctxt {
        fields_read: HashSet::new(),
        fields_written: HashSet::new(),
        requires: Vec::new(),
        ensures: Vec::new(),
        ident_to_field,
    };

    let mut tr = tr.clone();
    walk_translate_expressions(&mut ctxt, &mut tr.body)?;
    exchange_collect(&mut ctxt, &tr.body, Vec::new(), Vec::new())?;

    let mut in_args: Vec<TokenStream> = Vec::new();

    for field in &sm.fields {
        let is_output = ctxt.fields_written.contains(&field.ident);
        let is_input = is_output || ctxt.fields_read.contains(&field.ident);

        if !is_input {
            continue;
        }

        let arg_name = transition_arg_name(field);
        let arg_type = field_token_type_name(&sm.name, field);

        if is_output {
            let e_opt = get_output_value_for_variable(&tr.body, field);
            let e = e_opt.expect("get_output_value_for_variable");
            let lhs = get_new_field_value(field);
            let eq_e = Expr::Verbatim(quote! { ::builtin::equal(#lhs, #e) });
            ctxt.ensures.push(eq_e);

            in_args.push(quote! { #[proof] #arg_name: &mut #arg_type });
        } else {
            in_args.push(quote! { #[proof] #arg_name: &#arg_type });
        }
    }

    let reqs = ctxt.requires;
    let enss = ctxt.ensures;
    let exch_name = exchange_name(&sm.name, &tr);

    let req_stream = if reqs.len() > 0 {
        quote!{
            ::builtin::requires([
                #(#reqs),*
            ]);
        }
    } else {
        TokenStream::new()
    };

    let ens_stream = if enss.len() > 0 {
        quote!{
            ::builtin::ensures([
                #(#enss),*
            ]);
        }
    } else {
        TokenStream::new()
    };

    return Ok(quote! {
        #[proof]
        #[verifier(external_body)]
        pub fn #exch_name(#(#in_args),*) {
            #req_stream
            #ens_stream
        }
    });
}

// Translate expressions

fn walk_translate_expressions(
    ctxt: &mut Ctxt,
    ts: &mut TransitionStmt<Span, Ident, Expr>,
) -> syn::parse::Result<()> {
    match ts {
        TransitionStmt::Block(_span, v) => {
            for child in v.iter_mut() {
                walk_translate_expressions(ctxt, child)?;
            }
            Ok(())
        }
        TransitionStmt::Let(_span, _id, e) => {
            let init_e = translate_expr(ctxt, e)?;
            *e = init_e;
            Ok(())
        }
        TransitionStmt::If(_span, cond, e1, e2) => {
            let cond_e = translate_expr(ctxt, cond)?;
            *cond = cond_e;
            walk_translate_expressions(ctxt, e1)?;
            walk_translate_expressions(ctxt, e2)?;
            Ok(())
        }
        TransitionStmt::Require(_span, e) => {
            let req_e = translate_expr(ctxt, e)?;
            *e = req_e;
            Ok(())
        }
        TransitionStmt::Assert(_span, e) => {
            let assert_e = translate_expr(ctxt, e)?;
            *e = assert_e;
            Ok(())
        }
        TransitionStmt::Update(_span, _id, e) => {
            let update_e = translate_expr(ctxt, e)?;
            *e = update_e;
            Ok(())
        }
    }
}

fn translate_expr(ctxt: &mut Ctxt, expr: &Expr) -> syn::parse::Result<Expr> {
    let mut v = TranslatorVisitor::new(ctxt);
    let mut e = expr.clone();
    v.visit_expr_mut(&mut e);
    if v.errors.len() > 0 {
        return Err(v.errors[0].clone()); // TODO report all errors?
    }
    Ok(e)
}

struct TranslatorVisitor<'a> {
    pub errors: Vec<Error>,
    pub ctxt: &'a mut Ctxt,
}

impl<'a> TranslatorVisitor<'a> {
    pub fn new(ctxt: &'a mut Ctxt) -> TranslatorVisitor<'a> {
        TranslatorVisitor { errors: Vec::new(), ctxt: ctxt }
    }
}

impl<'a> VisitMut for TranslatorVisitor<'a> {
    fn visit_expr_mut(&mut self, node: &mut Expr) {
        let span = node.span();
        match node {
            Expr::Path(ExprPath { attrs: _, qself: None, path }) if path.is_ident("self") => {
                self.errors.push(Error::new(span,
                    "in a concurrent state machine, 'self' cannot be used opaquely; it may only be used by accessing its fields"));
            }
            Expr::Field(ExprField {
                base: box Expr::Path(ExprPath { attrs: _, qself: None, path }),
                member,
                attrs: _,
                dot_token: _,
            }) if path.is_ident("self") => match member {
                Member::Named(ident) => match self.ctxt.get_field_by_ident(span, ident) {
                    Err(err) => self.errors.push(err),
                    Ok(field) => {
                        self.ctxt.mark_field_as_read(&field);
                        match &field.stype {
                            ShardableType::Variable(_ty) => {
                                *node = get_old_field_value(&field);
                            }
                        }
                    }
                },
                _ => {
                    self.errors.push(Error::new(span, "expected a named field"));
                }
            },
            _ => syn::visit_mut::visit_expr_mut(self, node),
        }
    }
}

fn get_old_field_value(field: &Field<Ident, Type>) -> Expr {
    let arg = transition_arg_name(&field);
    let field = field_token_field_name(&field);
    Expr::Verbatim(quote! { ::builtin::old(#arg).#field })
}

fn get_new_field_value(field: &Field<Ident, Type>) -> Expr {
    let arg = transition_arg_name(&field);
    let field = field_token_field_name(&field);
    Expr::Verbatim(quote! { #arg.#field })
}

// Collect requires and ensures

#[derive(Clone, Debug)]
enum PrequelElement {
    Condition(Expr),
    Let(Ident, Expr),
    Branch(Expr, Vec<PrequelElement>, Vec<PrequelElement>),
}

fn exchange_collect(
    ctxt: &mut Ctxt,
    ts: &TransitionStmt<Span, Ident, Expr>,
    prequel: Vec<PrequelElement>,
    prequel_with_asserts: Vec<PrequelElement>,
) -> syn::parse::Result<(Vec<PrequelElement>, Vec<PrequelElement>)> {
    match ts {
        TransitionStmt::Block(_span, v) => {
            let mut p = prequel;
            let mut pa = prequel_with_asserts;
            for child in v.iter() {
                let (p1, pa1) = exchange_collect(ctxt, child, p, pa)?;
                p = p1;
                pa = pa1;
            }
            Ok((p, pa))
        }
        TransitionStmt::Let(_span, id, init_e) => {
            let mut p = prequel;
            let mut pa = prequel_with_asserts;
            let el = PrequelElement::Let(id.clone(), init_e.clone());
            p.push(el.clone());
            pa.push(el);
            Ok((p, pa))
        }
        TransitionStmt::If(_span, cond_e, e1, e2) => {
            let cond = PrequelElement::Condition(cond_e.clone());
            let not_cond = PrequelElement::Condition(bool_not_expr(cond_e));

            let mut p1 = prequel.clone();
            let mut pa1 = prequel_with_asserts.clone();
            p1.push(cond.clone());
            pa1.push(cond);
            let (_p1, pa1) = exchange_collect(ctxt, e1, p1, pa1)?;

            let mut p2 = prequel.clone();
            let mut pa2 = prequel_with_asserts.clone();
            p2.push(not_cond.clone());
            pa2.push(not_cond);
            let (_p2, pa2) = exchange_collect(ctxt, e2, p2, pa2)?;

            let l = prequel_with_asserts.len();
            let joined_pa = join_with_conditional(
                prequel_with_asserts,
                cond_e.clone(),
                pa1[l + 1..].to_vec(),
                pa2[l + 1..].to_vec(),
            );

            Ok((prequel, joined_pa))
        }
        TransitionStmt::Require(_span, req_e) => {
            ctxt.requires.push(with_prequel(&prequel_with_asserts, req_e.clone()));
            Ok((prequel, prequel_with_asserts))
        }
        TransitionStmt::Assert(_span, assert_e) => {
            ctxt.ensures.push(with_prequel(&prequel, assert_e.clone()));
            let mut pa = prequel_with_asserts;
            pa.push(PrequelElement::Condition(assert_e.clone()));
            Ok((prequel, pa))
        }
        TransitionStmt::Update(_span, id, _e) => {
            ctxt.fields_written.insert(id.clone());
            Ok((prequel, prequel_with_asserts))
        }
    }
}

fn join_with_conditional(
    base: Vec<PrequelElement>,
    cond: Expr,
    v1: Vec<PrequelElement>,
    v2: Vec<PrequelElement>,
) -> Vec<PrequelElement> {
    let mut b = base;
    b.push(PrequelElement::Branch(cond, v1, v2));
    b
}

fn bool_not_expr(e: &Expr) -> Expr {
    Expr::Verbatim(quote! { !(#e) })
}

fn with_prequel(pre: &Vec<PrequelElement>, e: Expr) -> Expr {
    let mut e = e;
    for p in pre.iter().rev() {
        match p {
            PrequelElement::Let(id, init_e) => {
                e = Expr::Verbatim(quote! { { let #id = #init_e; #e } });
            }
            PrequelElement::Condition(cond_e) => {
                e = Expr::Verbatim(quote! { ((#cond_e) >>= (#e)) });
            }
            PrequelElement::Branch(_, _, _) => {
                let cond_e = prequel_element_to_expr(p);
                if let Some(ce) = cond_e {
                    e = Expr::Verbatim(quote! { (#ce >>= #e) });
                }
            }
        }
    }
    e
}

fn prequel_element_to_expr(p: &PrequelElement) -> Option<Expr> {
    match p {
        PrequelElement::Condition(e) => Some(e.clone()),
        PrequelElement::Let(_, _) => None,
        PrequelElement::Branch(b, v1, v2) => {
            let e1 = prequel_vec_to_expr(v1);
            let e2 = prequel_vec_to_expr(v2);
            match (e1, e2) {
                (None, None) => None,
                (Some(e1), None) => Some(Expr::Verbatim(quote! { ((#b) >>= (#e1)) })),
                (None, Some(e2)) => Some(Expr::Verbatim(quote! { (!(#b) >>= (#e2)) })),
                (Some(e1), Some(e2)) => Some(Expr::Verbatim(quote! { (if #b { #e1 } else { #e2 }) })),
            }
        }
    }
}

fn prequel_vec_to_expr(v: &Vec<PrequelElement>) -> Option<Expr> {
    let mut opt = None;
    for p in v.iter().rev() {
        match p {
            PrequelElement::Let(id, init_e) => {
                if let Some(o) = opt {
                    opt = Some(Expr::Verbatim(quote! { { let #id = #init_e; #o } }));
                }
            }
            PrequelElement::Condition(cond_e) => match opt {
                None => {
                    opt = Some(Expr::Verbatim(quote! { (#cond_e) }));
                }
                Some(e) => {
                    opt = Some(Expr::Verbatim(quote! { ((#cond_e) && #e) }));
                }
            },
            PrequelElement::Branch(_, _, _) => {
                let cond_e = prequel_element_to_expr(p);
                if let Some(ce) = cond_e {
                    if let Some(o) = opt {
                        opt = Some(Expr::Verbatim(quote! { (#ce && #o) }));
                    } else {
                        opt = Some(ce);
                    }
                }
            }
        }
    }
    opt
}

fn get_output_value_for_variable(
    ts: &TransitionStmt<Span, Ident, Expr>,
    field: &Field<Ident, Type>,
) -> Option<Expr> {
    match ts {
        TransitionStmt::Block(_span, v) => {
            let mut opt = None;
            for child in v.iter() {
                let o = get_output_value_for_variable(child, field);
                if o.is_some() {
                    assert!(!opt.is_some());
                    opt = o;
                }
            }
            opt
        }
        TransitionStmt::Let(_, _, _)
        | TransitionStmt::Require(_, _)
        | TransitionStmt::Assert(_, _) => None,
        TransitionStmt::If(_span, cond_e, e1, e2) => {
            let o1 = get_output_value_for_variable(e1, field);
            let o2 = get_output_value_for_variable(e2, field);
            if o1.is_none() && o2.is_none() {
                None
            } else {
                let e1 = match o1 {
                    None => get_old_field_value(&field),
                    Some(e) => e,
                };
                let e2 = match o2 {
                    None => get_old_field_value(&field),
                    Some(e) => e,
                };
                Some(Expr::Verbatim(quote! { if #cond_e { #e1 } else { #e2 } }))
            }
        }
        TransitionStmt::Update(_span, id, e) => {
            if *id.to_string() == *field.ident.to_string() {
                Some(e.clone())
            } else {
                None
            }
        }
    }
}