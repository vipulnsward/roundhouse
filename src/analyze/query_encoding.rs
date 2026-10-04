use crate::expr::{Expr, ExprNode};
use crate::ty::Ty;

pub(crate) fn supported_call(receiver: &Expr, args: &[Expr], has_block: bool) -> bool {
    !has_block
        && (args.is_empty()
            || matches!(args, [arg] if matches!(arg.ty.as_ref(), Some(Ty::Str | Ty::Nil))))
        && matches!(receiver.ty, Some(Ty::Hash { .. }))
        && supported_value(receiver)
}

fn supported_value(expr: &Expr) -> bool {
    match &*expr.node {
        ExprNode::Hash { entries, .. } => entries.iter().all(|(key, value)| {
            key.ty.as_ref().is_some_and(supported_key) && supported_value(value)
        }),
        ExprNode::Array { elements, .. } => elements.iter().all(supported_value),
        _ => expr.ty.as_ref().is_some_and(supported_type),
    }
}

fn supported_type(ty: &Ty) -> bool {
    match ty {
        Ty::Str | Ty::Sym | Ty::Int | Ty::Float | Ty::Bool | Ty::Nil => true,
        Ty::Hash { key, value } => supported_key(key) && supported_type(value),
        Ty::Array { elem } => supported_type(elem),
        Ty::Union { variants } => !variants.is_empty() && variants.iter().all(supported_type),
        _ => false,
    }
}

fn supported_key(ty: &Ty) -> bool {
    match ty {
        Ty::Str | Ty::Sym => true,
        Ty::Union { variants } => !variants.is_empty() && variants.iter().all(supported_key),
        _ => false,
    }
}

pub(crate) fn contains_unlowered_call(expr: &Expr) -> bool {
    if matches!(&*expr.node, ExprNode::Send { recv: Some(recv), method, .. }
        if method.as_str() == "to_query"
            && (matches!(recv.ty, Some(Ty::Hash { .. })) || matches!(&*recv.node, ExprNode::Hash { .. })))
    {
        return true;
    }
    let mut found = false;
    expr.node
        .for_each_child(&mut |child| found |= contains_unlowered_call(child));
    found
}
