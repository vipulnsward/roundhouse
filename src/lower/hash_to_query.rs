use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::ty::Ty;

macro_rules! query_roots {
    ($app:ident, $f:ident, $iter:ident, $values:ident, $option:ident $(, $mutable:tt)?) => {
        for view in & $($mutable)? $app.views {
            for default in view.strict_locals.$iter().flatten().filter_map(|p| p.default.$option()) { $f(default); }
        }
        for controller in & $($mutable)? $app.controllers {
            for item in & $($mutable)? controller.body {
                if let crate::dialect::ControllerBodyItem::Action { action, .. } = item {
                    for default in action.kw_params.$iter().filter_map(|(_, e)| e.$option()) { $f(default); }
                }
            }
        }
        for fixture in & $($mutable)? $app.fixtures {
            for e in & $($mutable)? fixture.preamble { $f(e); }
            for value in fixture.records.$values().flat_map(|record| record.$values()) {
                if let crate::dialect::FixtureValue::Ruby(e) = value { $f(e); }
            }
        }
        for helper in & $($mutable)? $app.routes.direct_helpers { $f(& $($mutable)? helper.body); }
        for function in & $($mutable)? $app.sql_functions {
            let methods = match & $($mutable)? function.kind {
                crate::app::SqlFunctionKind::Scalar { method } => vec![method],
                crate::app::SqlFunctionKind::Aggregate { step, finalize } => vec![step, finalize],
            };
            for method in methods {
                $f(& $($mutable)? method.body);
                for default in method.params.$iter().filter_map(|p| p.default.$option()) { $f(default); }
            }
        }
    }
}

pub fn apply_hash_to_query_lowering(app: &mut App) {
    super::for_each_forwarding_body(app, &mut rewrite);
    let rewrite_extra = rewrite;
    query_roots!(app, rewrite_extra, iter_mut, values_mut, as_mut, mut);
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let span = expr.span;
    let ExprNode::Send {
        recv,
        method,
        args,
        block,
        parenthesized,
    } = &mut *expr.node
    else {
        return;
    };
    if method.as_str() != "to_query"
        || !recv.as_ref().is_some_and(|r| {
            crate::analyze::query_encoding::supported_call(r, args, block.is_some())
        })
    {
        return;
    }
    let receiver = recv.take().expect("typed hash receiver");
    *recv = Some(Expr::new(
        span,
        ExprNode::Const {
            path: vec![Symbol::from("ActionView"), Symbol::from("ViewHelpers")],
        },
    ));
    *method = Symbol::from("hash_to_query");
    args.insert(0, receiver);
    *parenthesized = true;
    expr.ty = Some(Ty::Str);
}

pub(crate) fn contains_call(expr: &Expr) -> bool {
    if matches!(&*expr.node, ExprNode::Send { recv: Some(recv), method, .. }
        if method.as_str() == "hash_to_query"
            && matches!(&*recv.node, ExprNode::Const { path }
                if path.iter().map(Symbol::as_str).eq(["ActionView", "ViewHelpers"])))
    {
        return true;
    }
    let mut found = false;
    expr.node
        .for_each_child(&mut |child| found |= contains_call(child));
    found
}

pub(crate) fn contains_app_call(app: &App) -> bool {
    any_app_call(app, contains_call)
}

pub(crate) fn contains_unlowered_app_call(app: &App) -> bool {
    any_app_call(app, crate::analyze::query_encoding::contains_unlowered_call)
}

fn any_app_call(app: &App, predicate: fn(&Expr) -> bool) -> bool {
    let mut found = false;
    let mut visit = |expr: &Expr| found |= predicate(expr);
    super::for_each_forwarding_body_ref(app, &mut visit);
    query_roots!(app, visit, iter, values, as_ref);
    found
}
