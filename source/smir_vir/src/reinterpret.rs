use air::ast::Span;
use air::errors::error;
use smir::ast::{Arg, Transition, TransitionKind, TransitionStmt};
use vir::ast::{
    CallTarget, Expr, ExprX, Function, Ident, Mode, PathX, PatternX, Stmt, StmtX, Typ, UnaryOpr,
    VirErr,
};

pub fn reinterpret_func_as_transition(
    f: Function,
    kind: TransitionKind,
) -> Result<Transition<Span, Ident, Expr, Typ>, VirErr> {
    let vir_body = match &f.x.body {
        None => { return Err(error("unsupported: opaque transition", &f.span)); }
        Some(body) /* once told me */ => body,
    };

    let mut args = Vec::new();
    for p in f.x.params.iter() {
        args.push(Arg { ident: p.x.name.clone(), ty: p.x.typ.clone() });
    }

    let body = get_body_from_expr(&vir_body)?;

    let name = f.x.name.path.segments.last().expect("last segment").clone();

    Ok(Transition { kind, args, body, name })
}

fn get_body_from_expr(e: &Expr) -> Result<TransitionStmt<Span, Ident, Expr>, VirErr> {
    match &e.x {
        ExprX::If(cond, thn_body, els_body) => {
            let thn = get_body_from_expr(thn_body)?;
            let els = match els_body {
                Some(els_body) => get_body_from_expr(els_body)?,
                None => TransitionStmt::Block(e.span.clone(), Vec::new()),
            };
            return Ok(TransitionStmt::If(
                e.span.clone(),
                cond.clone(),
                Box::new(thn),
                Box::new(els),
            ));
        }
        ExprX::Block(stmts, end_e) => {
            match end_e {
                Some(e) => {
                    return Err(error("unsupported expression return in transition", &e.span));
                }
                None => {}
            }
            let mut v = Vec::new();
            for stmt in stmts.iter() {
                v.push(get_body_from_stmt(stmt)?);
            }
            Ok(TransitionStmt::Block(e.span.clone(), v))
        }
        ExprX::Call(target, args) => {
            let op_name = target_to_op_name(target, &e.span)?;
            match op_name.as_str() {
                "assert" => {
                    assert!(args.len() == 1);
                    return Ok(TransitionStmt::Assert(e.span.clone(), args[0].clone()));
                }
                "require" => {
                    assert!(args.len() == 1);
                    return Ok(TransitionStmt::Require(e.span.clone(), args[0].clone()));
                }
                "update" => {
                    assert!(args.len() == 2);
                    let ident = get_update_ident(&args[0])?;
                    return Ok(TransitionStmt::Update(e.span.clone(), ident, args[1].clone()));
                }
                _ => {
                    return Err(error("unsupported call", &e.span));
                }
            }
        }
        _ => {
            return Err(error("unsupported expression type in transition", &e.span));
        }
    }
}

fn get_body_from_stmt(s: &Stmt) -> Result<TransitionStmt<Span, Ident, Expr>, VirErr> {
    match &s.x {
        StmtX::Expr(e) => get_body_from_expr(e),
        StmtX::Decl { pattern, mode, init: Some(init) } => {
            if *mode != Mode::Spec {
                return Err(error("only spec variables alllowed in transition", &s.span));
            }
            match &pattern.x {
                PatternX::Var { name, mutable: false } => {
                    return Ok(TransitionStmt::Let(s.span.clone(), name.clone(), init.clone()));
                }
                _ => {
                    return Err(error("unsupported statement type in transition", &s.span));
                }
            }
        }
        _ => {
            return Err(error("unsupported statement type in transition", &s.span));
        }
    }
}

fn target_to_op_name(target: &CallTarget, span: &Span) -> Result<String, VirErr> {
    match target {
        CallTarget::Static(fun, _) => match &*fun.path {
            PathX { krate: Some(builtin_ident), segments } if **builtin_ident == "builtin" => {
                match &segments[..] {
                    [state_machine_ops_ident, s_ident]
                        if **state_machine_ops_ident == "state_machine_ops" =>
                    {
                        Ok((**s_ident).clone())
                    }
                    _ => {
                        return Err(error("unsupported call", span));
                    }
                }
            }
            _ => {
                return Err(error("unsupported call", span));
            }
        },
        _ => {
            return Err(error("unsupported call", span));
        }
    }
}

fn get_update_ident(e: &Expr) -> Result<Ident, VirErr> {
    match &e.x {
        ExprX::UnaryOpr(UnaryOpr::Field { field: f, .. }, _) => Ok(f.clone()),
        _ => {
            return Err(error("unsupported arg", &e.span));
        }
    }
}