use std::collections::{BTreeSet, HashMap, HashSet};

use super::Analyzer;
use crate::App;
use crate::expr::{Expr, ExprNode};
use crate::ty::Ty;

impl Analyzer {
    pub(super) fn harvest_source_yields(&mut self, app: &App) {
        let record_methods: HashSet<_> = app
            .models
            .iter()
            .filter(|model| {
                model.parent.as_ref().is_some_and(|parent| {
                    matches!(parent.0.as_str(), "ApplicationRecord" | "ActiveRecord::Base")
                })
            })
            .flat_map(|model| {
                ["find"]
                    .into_iter()
                    .filter(|name| {
                        !app.models
                            .iter()
                            .any(|m| m.methods().any(|method| method.name.as_str() == *name))
                            && !app
                                .library_classes
                                .iter()
                                .any(|c| c.methods.iter().any(|method| method.name.as_str() == *name))
                    })
                    .map(|name| (model.name.clone(), crate::Symbol::from(name)))
            })
            .collect();
        for class in &app.library_classes {
            let mut inferred = HashMap::new();
            let Some(registered) = self.classes.get(&class.name) else {
                continue;
            };
            for method in &class.methods {
                if method.signature.is_some() || method.name.as_str() == "initialize" {
                    continue;
                }
                let mut instance = false;
                let mut class_method = false;
                let mut cursor = Some(&class.name);
                let mut seen = HashSet::new();
                let mut cycle = false;
                while let Some(id) = cursor {
                    if !seen.insert(id) {
                        cycle = true;
                        break;
                    }
                    let Some(ancestor) = self.classes.get(id) else {
                        break;
                    };
                    instance |= ancestor.instance_methods.contains_key(&method.name);
                    class_method |= ancestor.class_methods.contains_key(&method.name);
                    cursor = ancestor.parent.as_ref();
                }
                if cycle || (instance && class_method) {
                    continue;
                }
                let table = match method.receiver {
                    crate::dialect::MethodReceiver::Instance => &registered.instance_methods,
                    crate::dialect::MethodReceiver::Class => &registered.class_methods,
                };
                if matches!(table.get(&method.name), Some(Ty::Fn { .. })) {
                    continue;
                }
                if let Some(params) = yielded_params(
                    &method.body,
                    &method.params.iter().map(|p| p.name.clone()).collect(),
                    &record_methods,
                    &self.classes,
                    &class.name,
                ) {
                    inferred.insert(method.name.clone(), params);
                }
            }
            if let Some(registered) = self.classes.get_mut(&class.name) {
                registered.inferred_block_params = inferred;
            }
        }
    }
}

fn supported(ty: &Ty) -> bool {
    match ty {
        Ty::Nil | Ty::Str | Ty::Sym | Ty::Int | Ty::Float | Ty::Bool => true,
        Ty::Class { args, .. } => args.iter().all(supported),
        Ty::Union { variants } => !variants.is_empty() && variants.iter().all(supported),
        _ => false,
    }
}

fn yielded_params(
    body: &Expr,
    parameters: &BTreeSet<crate::Symbol>,
    record_methods: &HashSet<(crate::ClassId, crate::Symbol)>,
    classes: &HashMap<crate::ClassId, super::body::ClassInfo>,
    owner: &crate::ClassId,
) -> Option<Vec<Ty>> {
    fn class_argument_unproved(
        expr: &Expr,
        names: &BTreeSet<crate::Symbol>,
        record_methods: &HashSet<(crate::ClassId, crate::Symbol)>,
    ) -> bool {
        match &*expr.node {
            ExprNode::Const { .. } | ExprNode::SelfRef | ExprNode::Ivar { .. } => true,
            ExprNode::Var { name, .. } => names.contains(name),
            ExprNode::Send {
                recv: Some(recv),
                method,
                args,
                block: None,
                ..
            } if args.len() == 1 && matches!(args[0].ty, Some(Ty::Int)) => {
                !matches!(&*recv.node, ExprNode::Const { path } if record_methods.contains(&(crate::ClassId(crate::Symbol::from(path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::").as_str())), method.clone())))
            }
            _ => true,
        }
    }
    fn aliases(
        expr: &Expr,
        names: &mut BTreeSet<crate::Symbol>,
        record_methods: &HashSet<(crate::ClassId, crate::Symbol)>,
        classes: &HashMap<crate::ClassId, super::body::ClassInfo>,
        owner: &crate::ClassId,
    ) {
        if let ExprNode::Assign {
            target: crate::expr::LValue::Var { name, .. },
            value,
        } = &*expr.node
        {
            if class_argument_unproved(value, names, record_methods) {
                names.insert(name.clone());
            }
        }
        if let ExprNode::OpAssign {
            target: crate::expr::LValue::Var { name, .. },
            ..
        } = &*expr.node
        {
            names.insert(name.clone());
        }
        if let ExprNode::MultiAssign { targets, .. } = &*expr.node {
            names.extend(targets.iter().filter_map(|target| match target {
                crate::expr::LValue::Var { name, .. } => Some(name.clone()),
                _ => None,
            }));
        }
        if let ExprNode::Send {
            recv,
            method,
            block: Some(block),
            ..
        } = &*expr.node
        {
            let class = match recv.as_ref().map(|r| &*r.node) {
                None | Some(ExprNode::SelfRef) => Some(owner.clone()),
                Some(ExprNode::Const { path }) => Some(crate::ClassId(crate::Symbol::from(
                    path.iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join("::")
                        .as_str(),
                ))),
                _ => None,
            };
            let proven = class
                .and_then(|id| classes.get(&id))
                .is_some_and(|c| c.inferred_block_params.contains_key(method));
            if !proven {
                if let ExprNode::Lambda { params, .. } = &*block.node {
                    names.extend(params.iter().cloned());
                }
            }
        }
        expr.node
            .for_each_child(&mut |child| aliases(child, names, record_methods, classes, owner));
    }
    fn class_type(ty: &Ty) -> bool {
        matches!(ty, Ty::Class { .. })
            || matches!(ty, Ty::Union { variants } if variants.iter().any(class_type))
    }
    fn collect(
        expr: &Expr,
        names: &BTreeSet<crate::Symbol>,
        record_methods: &HashSet<(crate::ClassId, crate::Symbol)>,
        out: &mut Vec<Vec<Ty>>,
        unsupported: &mut bool,
    ) {
        if let ExprNode::Yield { args } = &*expr.node {
            let mut types = Vec::new();
            for arg in args {
                if matches!(
                    &*arg.node,
                    ExprNode::Splat { .. }
                        | ExprNode::KeywordSplat { .. }
                        | ExprNode::Const { .. }
                        | ExprNode::SelfRef
                ) {
                    *unsupported = true;
                    return;
                }
                let Some(ty) = arg.ty.clone().filter(supported) else {
                    *unsupported = true;
                    return;
                };
                if class_type(&ty) && class_argument_unproved(arg, names, record_methods) {
                    *unsupported = true;
                    return;
                }
                types.push(ty);
            }
            out.push(types);
        }
        expr.node
            .for_each_child(&mut |child| collect(child, names, record_methods, out, unsupported));
    }
    let mut yields = Vec::new();
    let mut unsupported = false;
    let mut names = parameters.clone();
    loop {
        let count = names.len();
        aliases(body, &mut names, record_methods, classes, owner);
        if names.len() == count {
            break;
        }
    }
    collect(body, &names, record_methods, &mut yields, &mut unsupported);
    if unsupported || yields.is_empty() {
        return None;
    }
    let count = yields.iter().map(Vec::len).max().unwrap_or(0);
    Some(
        (0..count)
            .map(|i| {
                yields
                    .iter()
                    .map(|types| types.get(i).cloned().unwrap_or(Ty::Nil))
                    .reduce(super::union_of)
                    .unwrap()
            })
            .collect(),
    )
}
