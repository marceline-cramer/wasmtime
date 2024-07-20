//! Generate Rust code from a series of Sequences.

use crate::sema::{ExternalSig, ReturnKind, Sym, Term, TermEnv, TermId, Type, TypeEnv, TypeId};
use crate::serialize::{Block, ControlFlow, EvalStep, MatchArm};
use crate::stablemapset::StableSet;
use crate::trie_again::{Binding, BindingId, Constraint, RuleSet};
use std::fmt::Write;
use std::slice::Iter;

/// Options for code generation.
#[derive(Clone, Debug, Default)]
pub struct CodegenOptions {
    /// Do not include the `#![allow(...)]` pragmas in the generated
    /// source. Useful if it must be include!()'d elsewhere.
    pub exclude_global_allow_pragmas: bool,
}

/// Emit Rust source code for the given type and term environments.
pub fn codegen(
    typeenv: &TypeEnv,
    termenv: &TermEnv,
    terms: &[(TermId, RuleSet)],
    options: &CodegenOptions,
) -> String {
    Codegen::compile(typeenv, termenv, terms).generate_rust(options)
}

#[derive(Clone, Debug)]
struct Codegen<'a> {
    typeenv: &'a TypeEnv,
    termenv: &'a TermEnv,
    terms: &'a [(TermId, RuleSet)],
}

enum Nested<'a> {
    Cases(Iter<'a, EvalStep>),
    Arms(BindingId, Iter<'a, MatchArm>),
}

struct BodyContext<'a, W> {
    out: &'a mut W,
    ruleset: &'a RuleSet,
    indent: String,
    is_ref: StableSet<BindingId>,
    is_bound: StableSet<BindingId>,
}

impl<'a, W: Write> BodyContext<'a, W> {
    fn new(out: &'a mut W, ruleset: &'a RuleSet) -> Self {
        Self {
            out,
            ruleset,
            indent: Default::default(),
            is_ref: Default::default(),
            is_bound: Default::default(),
        }
    }

    fn enter_scope(&mut self) -> StableSet<BindingId> {
        let new = self.is_bound.clone();
        std::mem::replace(&mut self.is_bound, new)
    }

    fn begin_block(&mut self) -> std::fmt::Result {
        self.indent.push_str("    ");
        writeln!(self.out, " {{")
    }

    fn end_block(&mut self, last_line: &str, scope: StableSet<BindingId>) -> std::fmt::Result {
        if !last_line.is_empty() {
            writeln!(self.out, "{}{}", &self.indent, last_line)?;
        }
        self.is_bound = scope;
        self.end_block_without_newline()?;
        writeln!(self.out)
    }

    fn end_block_without_newline(&mut self) -> std::fmt::Result {
        self.indent.truncate(self.indent.len() - 4);
        write!(self.out, "{}}}", &self.indent)
    }

    fn set_ref(&mut self, binding: BindingId, is_ref: bool) {
        if is_ref {
            self.is_ref.insert(binding);
        } else {
            debug_assert!(!self.is_ref.contains(&binding));
        }
    }
}

impl<'a> Codegen<'a> {
    fn compile(
        typeenv: &'a TypeEnv,
        termenv: &'a TermEnv,
        terms: &'a [(TermId, RuleSet)],
    ) -> Codegen<'a> {
        Codegen {
            typeenv,
            termenv,
            terms,
        }
    }

    fn generate_rust(&self, options: &CodegenOptions) -> String {
        let mut code = String::new();

        self.generate_header(&mut code, options);
        self.generate_ctx_trait(&mut code);
        self.generate_internal_types(&mut code);
        self.generate_internal_term_constructors(&mut code).unwrap();

        code
    }

    fn generate_header(&self, code: &mut String, options: &CodegenOptions) {
        writeln!(code, "// GENERATED BY ISLE. DO NOT EDIT!").unwrap();
        writeln!(code, "//").unwrap();
        writeln!(
            code,
            "// Generated automatically from the instruction-selection DSL code in:",
        )
        .unwrap();
        for file in &self.typeenv.filenames {
            writeln!(code, "// - {}", file).unwrap();
        }

        if !options.exclude_global_allow_pragmas {
            writeln!(
                code,
                "\n#![allow(dead_code, unreachable_code, unreachable_patterns)]"
            )
            .unwrap();
            writeln!(
                code,
                "#![allow(unused_imports, unused_variables, non_snake_case, unused_mut)]"
            )
            .unwrap();
            writeln!(
                code,
                "#![allow(irrefutable_let_patterns, unused_assignments, non_camel_case_types)]"
            )
            .unwrap();
        }

        writeln!(code, "\nuse super::*;  // Pulls in all external types.").unwrap();
        writeln!(code, "use std::marker::PhantomData;").unwrap();
    }

    fn generate_trait_sig(&self, code: &mut String, indent: &str, sig: &ExternalSig) {
        let ret_tuple = format!(
            "{open_paren}{rets}{close_paren}",
            open_paren = if sig.ret_tys.len() != 1 { "(" } else { "" },
            rets = sig
                .ret_tys
                .iter()
                .map(|&ty| self.type_name(ty, /* by_ref = */ false))
                .collect::<Vec<_>>()
                .join(", "),
            close_paren = if sig.ret_tys.len() != 1 { ")" } else { "" },
        );

        if sig.ret_kind == ReturnKind::Iterator {
            writeln!(
                code,
                "{indent}type {name}_returns: Default + IntoContextIter<Context = Self, Output = {output}>;",
                indent = indent,
                name = sig.func_name,
                output = ret_tuple,
            )
            .unwrap();
        }

        let ret_ty = match sig.ret_kind {
            ReturnKind::Plain => ret_tuple,
            ReturnKind::Option => format!("Option<{}>", ret_tuple),
            ReturnKind::Iterator => format!("()"),
        };

        writeln!(
            code,
            "{indent}fn {name}(&mut self, {params}) -> {ret_ty};",
            indent = indent,
            name = sig.func_name,
            params = sig
                .param_tys
                .iter()
                .enumerate()
                .map(|(i, &ty)| format!("arg{}: {}", i, self.type_name(ty, /* by_ref = */ true)))
                .chain(if sig.ret_kind == ReturnKind::Iterator {
                    Some(format!("returns: &mut Self::{}_returns", sig.func_name))
                } else {
                    None
                })
                .collect::<Vec<_>>()
                .join(", "),
            ret_ty = ret_ty,
        )
        .unwrap();
    }

    fn generate_ctx_trait(&self, code: &mut String) {
        writeln!(code).unwrap();
        writeln!(
            code,
            "/// Context during lowering: an implementation of this trait"
        )
        .unwrap();
        writeln!(
            code,
            "/// must be provided with all external constructors and extractors."
        )
        .unwrap();
        writeln!(
            code,
            "/// A mutable borrow is passed along through all lowering logic."
        )
        .unwrap();
        writeln!(code, "pub trait Context {{").unwrap();
        for term in &self.termenv.terms {
            if term.has_external_extractor() {
                let ext_sig = term.extractor_sig(self.typeenv).unwrap();
                self.generate_trait_sig(code, "    ", &ext_sig);
            }
            if term.has_external_constructor() {
                let ext_sig = term.constructor_sig(self.typeenv).unwrap();
                self.generate_trait_sig(code, "    ", &ext_sig);
            }
        }
        writeln!(code, "}}").unwrap();
        writeln!(
            code,
            r#"
pub trait ContextIter {{
    type Context;
    type Output;
    fn next(&mut self, ctx: &mut Self::Context) -> Option<Self::Output>;
    fn size_hint(&self) -> (usize, Option<usize>) {{ (0, None) }}
}}

pub trait IntoContextIter {{
    type Context;
    type Output;
    type IntoIter: ContextIter<Context = Self::Context, Output = Self::Output>;
    fn into_context_iter(self) -> Self::IntoIter;
}}

pub trait Length {{
    fn len(&self) -> usize;
}}

impl<T> Length for std::vec::Vec<T> {{
    fn len(&self) -> usize {{
        std::vec::Vec::len(self)
    }}
}}

pub struct ContextIterWrapper<I, C> {{
    iter: I,
    _ctx: std::marker::PhantomData<C>,
}}
impl<I: Default, C> Default for ContextIterWrapper<I, C> {{
    fn default() -> Self {{
        ContextIterWrapper {{
            iter: I::default(),
            _ctx: std::marker::PhantomData
        }}
    }}
}}
impl<I, C> std::ops::Deref for ContextIterWrapper<I, C> {{
    type Target = I;
    fn deref(&self) -> &I {{
        &self.iter
    }}
}}
impl<I, C> std::ops::DerefMut for ContextIterWrapper<I, C> {{
    fn deref_mut(&mut self) -> &mut I {{
        &mut self.iter
    }}
}}
impl<I: Iterator, C: Context> From<I> for ContextIterWrapper<I, C> {{
    fn from(iter: I) -> Self {{
        Self {{ iter, _ctx: std::marker::PhantomData }}
    }}
}}
impl<I: Iterator, C: Context> ContextIter for ContextIterWrapper<I, C> {{
    type Context = C;
    type Output = I::Item;
    fn next(&mut self, _ctx: &mut Self::Context) -> Option<Self::Output> {{
        self.iter.next()
    }}
    fn size_hint(&self) -> (usize, Option<usize>) {{
        self.iter.size_hint()
    }}
}}
impl<I: IntoIterator, C: Context> IntoContextIter for ContextIterWrapper<I, C> {{
    type Context = C;
    type Output = I::Item;
    type IntoIter = ContextIterWrapper<I::IntoIter, C>;
    fn into_context_iter(self) -> Self::IntoIter {{
        ContextIterWrapper {{
            iter: self.iter.into_iter(),
            _ctx: std::marker::PhantomData
        }}
    }}
}}
impl<T, E: Extend<T>, C> Extend<T> for ContextIterWrapper<E, C> {{
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {{
        self.iter.extend(iter);
    }}
}}
impl<L: Length, C> Length for ContextIterWrapper<L, C> {{
    fn len(&self) -> usize {{
        self.iter.len()
    }}
}}
           "#,
        )
        .unwrap();
    }

    fn generate_internal_types(&self, code: &mut String) {
        for ty in &self.typeenv.types {
            match ty {
                &Type::Enum {
                    name,
                    is_extern,
                    is_nodebug,
                    ref variants,
                    pos,
                    ..
                } if !is_extern => {
                    let name = &self.typeenv.syms[name.index()];
                    writeln!(
                        code,
                        "\n/// Internal type {}: defined at {}.",
                        name,
                        pos.pretty_print_line(&self.typeenv.filenames[..])
                    )
                    .unwrap();

                    // Generate the `derive`s.
                    let debug_derive = if is_nodebug { "" } else { ", Debug" };
                    if variants.iter().all(|v| v.fields.is_empty()) {
                        writeln!(
                            code,
                            "#[derive(Copy, Clone, PartialEq, Eq{})]",
                            debug_derive
                        )
                        .unwrap();
                    } else {
                        writeln!(code, "#[derive(Clone{})]", debug_derive).unwrap();
                    }

                    writeln!(code, "pub enum {} {{", name).unwrap();
                    for variant in variants {
                        let name = &self.typeenv.syms[variant.name.index()];
                        if variant.fields.is_empty() {
                            writeln!(code, "    {},", name).unwrap();
                        } else {
                            writeln!(code, "    {} {{", name).unwrap();
                            for field in &variant.fields {
                                let name = &self.typeenv.syms[field.name.index()];
                                let ty_name =
                                    self.typeenv.types[field.ty.index()].name(self.typeenv);
                                writeln!(code, "        {}: {},", name, ty_name).unwrap();
                            }
                            writeln!(code, "    }},").unwrap();
                        }
                    }
                    writeln!(code, "}}").unwrap();
                }
                _ => {}
            }
        }
    }

    fn type_name(&self, typeid: TypeId, by_ref: bool) -> String {
        match self.typeenv.types[typeid.index()] {
            Type::Primitive(_, sym, _) => self.typeenv.syms[sym.index()].clone(),
            Type::Enum { name, .. } => {
                let r = if by_ref { "&" } else { "" };
                format!("{}{}", r, self.typeenv.syms[name.index()])
            }
        }
    }

    fn generate_internal_term_constructors(&self, code: &mut String) -> std::fmt::Result {
        for &(termid, ref ruleset) in self.terms.iter() {
            let root = crate::serialize::serialize(ruleset);
            let mut ctx = BodyContext::new(code, ruleset);

            let termdata = &self.termenv.terms[termid.index()];
            let term_name = &self.typeenv.syms[termdata.name.index()];
            writeln!(ctx.out)?;
            writeln!(
                ctx.out,
                "{}// Generated as internal constructor for term {}.",
                &ctx.indent, term_name,
            )?;

            let sig = termdata.constructor_sig(self.typeenv).unwrap();
            writeln!(
                ctx.out,
                "{}pub fn {}<C: Context>(",
                &ctx.indent, sig.func_name
            )?;

            writeln!(ctx.out, "{}    ctx: &mut C,", &ctx.indent)?;
            for (i, &ty) in sig.param_tys.iter().enumerate() {
                let (is_ref, sym) = self.ty(ty);
                write!(ctx.out, "{}    arg{}: ", &ctx.indent, i)?;
                write!(
                    ctx.out,
                    "{}{}",
                    if is_ref { "&" } else { "" },
                    &self.typeenv.syms[sym.index()]
                )?;
                if let Some(binding) = ctx.ruleset.find_binding(&Binding::Argument {
                    index: i.try_into().unwrap(),
                }) {
                    ctx.set_ref(binding, is_ref);
                }
                writeln!(ctx.out, ",")?;
            }

            let (_, ret) = self.ty(sig.ret_tys[0]);
            let ret = &self.typeenv.syms[ret.index()];

            if let ReturnKind::Iterator = sig.ret_kind {
                writeln!(
                    ctx.out,
                    "{}    returns: &mut (impl Extend<{}> + Length),",
                    &ctx.indent, ret
                )?;
            }

            write!(ctx.out, "{}) -> ", &ctx.indent)?;
            match sig.ret_kind {
                ReturnKind::Iterator => write!(ctx.out, "()")?,
                ReturnKind::Option => write!(ctx.out, "Option<{}>", ret)?,
                ReturnKind::Plain => write!(ctx.out, "{}", ret)?,
            };

            let last_expr = if let Some(EvalStep {
                check: ControlFlow::Return { .. },
                ..
            }) = root.steps.last()
            {
                // If there's an outermost fallback, no need for another `return` statement.
                String::new()
            } else {
                match sig.ret_kind {
                    ReturnKind::Iterator => String::new(),
                    ReturnKind::Option => "None".to_string(),
                    ReturnKind::Plain => format!(
                        "unreachable!(\"no rule matched for term {{}} at {{}}; should it be partial?\", {:?}, {:?})",
                        term_name,
                        termdata
                            .decl_pos
                            .pretty_print_line(&self.typeenv.filenames[..])
                    ),
                }
            };

            let scope = ctx.enter_scope();
            self.emit_block(&mut ctx, &root, sig.ret_kind, &last_expr, scope)?;
        }
        Ok(())
    }

    fn ty(&self, typeid: TypeId) -> (bool, Sym) {
        match &self.typeenv.types[typeid.index()] {
            &Type::Primitive(_, sym, _) => (false, sym),
            &Type::Enum { name, .. } => (true, name),
        }
    }

    fn validate_block(ret_kind: ReturnKind, block: &Block) -> Nested {
        if !matches!(ret_kind, ReturnKind::Iterator) {
            // Loops are only allowed if we're returning an iterator.
            assert!(!block
                .steps
                .iter()
                .any(|c| matches!(c.check, ControlFlow::Loop { .. })));

            // Unless we're returning an iterator, a case which returns a result must be the last
            // case in a block.
            if let Some(result_pos) = block
                .steps
                .iter()
                .position(|c| matches!(c.check, ControlFlow::Return { .. }))
            {
                assert_eq!(block.steps.len() - 1, result_pos);
            }
        }

        Nested::Cases(block.steps.iter())
    }

    fn emit_block<W: Write>(
        &self,
        ctx: &mut BodyContext<W>,
        block: &Block,
        ret_kind: ReturnKind,
        last_expr: &str,
        scope: StableSet<BindingId>,
    ) -> std::fmt::Result {
        let mut stack = Vec::new();
        ctx.begin_block()?;
        stack.push((Self::validate_block(ret_kind, block), last_expr, scope));

        while let Some((mut nested, last_line, scope)) = stack.pop() {
            match &mut nested {
                Nested::Cases(cases) => {
                    let Some(case) = cases.next() else {
                        ctx.end_block(last_line, scope)?;
                        continue;
                    };
                    // Iterator isn't done, put it back on the stack.
                    stack.push((nested, last_line, scope));

                    for &expr in case.bind_order.iter() {
                        let iter_return = match &ctx.ruleset.bindings[expr.index()] {
                            Binding::Extractor { term, .. } => {
                                let termdata = &self.termenv.terms[term.index()];
                                let sig = termdata.extractor_sig(self.typeenv).unwrap();
                                if sig.ret_kind == ReturnKind::Iterator {
                                    if termdata.has_external_extractor() {
                                        Some(format!("C::{}_returns", sig.func_name))
                                    } else {
                                        Some(format!("ContextIterWrapper::<ConstructorVec<_>, _>"))
                                    }
                                } else {
                                    None
                                }
                            }
                            Binding::Constructor { term, .. } => {
                                let termdata = &self.termenv.terms[term.index()];
                                let sig = termdata.constructor_sig(self.typeenv).unwrap();
                                if sig.ret_kind == ReturnKind::Iterator {
                                    if termdata.has_external_constructor() {
                                        Some(format!("C::{}_returns", sig.func_name))
                                    } else {
                                        Some(format!("ContextIterWrapper::<ConstructorVec<_>, _>"))
                                    }
                                } else {
                                    None
                                }
                            }
                            _ => None,
                        };
                        if let Some(ty) = iter_return {
                            writeln!(
                                ctx.out,
                                "{}let mut v{} = {}::default();",
                                &ctx.indent,
                                expr.index(),
                                ty
                            )?;
                            write!(ctx.out, "{}", &ctx.indent)?;
                        } else {
                            write!(ctx.out, "{}let v{} = ", &ctx.indent, expr.index())?;
                        }
                        self.emit_expr(ctx, expr)?;
                        writeln!(ctx.out, ";")?;
                        ctx.is_bound.insert(expr);
                    }

                    match &case.check {
                        // Use a shorthand notation if there's only one match arm.
                        ControlFlow::Match { source, arms } if arms.len() == 1 => {
                            let arm = &arms[0];
                            let scope = ctx.enter_scope();
                            match arm.constraint {
                                Constraint::ConstInt { .. } | Constraint::ConstPrim { .. } => {
                                    write!(ctx.out, "{}if ", &ctx.indent)?;
                                    self.emit_expr(ctx, *source)?;
                                    write!(ctx.out, " == ")?;
                                    self.emit_constraint(ctx, *source, arm)?;
                                }
                                Constraint::Variant { .. } | Constraint::Some => {
                                    write!(ctx.out, "{}if let ", &ctx.indent)?;
                                    self.emit_constraint(ctx, *source, arm)?;
                                    write!(ctx.out, " = ")?;
                                    self.emit_source(ctx, *source, arm.constraint)?;
                                }
                            }
                            ctx.begin_block()?;
                            stack.push((Self::validate_block(ret_kind, &arm.body), "", scope));
                        }

                        ControlFlow::Match { source, arms } => {
                            let scope = ctx.enter_scope();
                            write!(ctx.out, "{}match ", &ctx.indent)?;
                            self.emit_source(ctx, *source, arms[0].constraint)?;
                            ctx.begin_block()?;

                            // Always add a catchall arm, because we
                            // don't do exhaustiveness checking on the
                            // match arms.
                            stack.push((Nested::Arms(*source, arms.iter()), "_ => {}", scope));
                        }

                        ControlFlow::Equal { a, b, body } => {
                            let scope = ctx.enter_scope();
                            write!(ctx.out, "{}if ", &ctx.indent)?;
                            self.emit_expr(ctx, *a)?;
                            write!(ctx.out, " == ")?;
                            self.emit_expr(ctx, *b)?;
                            ctx.begin_block()?;
                            stack.push((Self::validate_block(ret_kind, body), "", scope));
                        }

                        ControlFlow::Loop { result, body } => {
                            let source = match &ctx.ruleset.bindings[result.index()] {
                                Binding::Iterator { source } => source,
                                _ => unreachable!("Loop from a non-Iterator"),
                            };
                            let scope = ctx.enter_scope();

                            writeln!(
                                ctx.out,
                                "{}let mut v{} = v{}.into_context_iter();",
                                &ctx.indent,
                                source.index(),
                                source.index(),
                            )?;

                            write!(
                                ctx.out,
                                "{}while let Some(v{}) = v{}.next(ctx)",
                                &ctx.indent,
                                result.index(),
                                source.index()
                            )?;
                            ctx.is_bound.insert(*result);
                            ctx.begin_block()?;
                            stack.push((Self::validate_block(ret_kind, body), "", scope));
                        }

                        &ControlFlow::Return { pos, result } => {
                            writeln!(
                                ctx.out,
                                "{}// Rule at {}.",
                                &ctx.indent,
                                pos.pretty_print_line(&self.typeenv.filenames)
                            )?;
                            write!(ctx.out, "{}", &ctx.indent)?;
                            match ret_kind {
                                ReturnKind::Plain | ReturnKind::Option => {
                                    write!(ctx.out, "return ")?
                                }
                                ReturnKind::Iterator => write!(ctx.out, "returns.extend(Some(")?,
                            }
                            self.emit_expr(ctx, result)?;
                            if ctx.is_ref.contains(&result) {
                                write!(ctx.out, ".clone()")?;
                            }
                            match ret_kind {
                                ReturnKind::Plain | ReturnKind::Option => writeln!(ctx.out, ";")?,
                                ReturnKind::Iterator => {
                                    writeln!(ctx.out, "));")?;
                                    writeln!(
                                        ctx.out,
                                        "{}if returns.len() >= MAX_ISLE_RETURNS {{ return; }}",
                                        ctx.indent
                                    )?;
                                }
                            }
                        }
                    }
                }

                Nested::Arms(source, arms) => {
                    let Some(arm) = arms.next() else {
                        ctx.end_block(last_line, scope)?;
                        continue;
                    };
                    let source = *source;
                    // Iterator isn't done, put it back on the stack.
                    stack.push((nested, last_line, scope));

                    let scope = ctx.enter_scope();
                    write!(ctx.out, "{}", &ctx.indent)?;
                    self.emit_constraint(ctx, source, arm)?;
                    write!(ctx.out, " =>")?;
                    ctx.begin_block()?;
                    stack.push((Self::validate_block(ret_kind, &arm.body), "", scope));
                }
            }
        }

        Ok(())
    }

    fn emit_expr<W: Write>(&self, ctx: &mut BodyContext<W>, result: BindingId) -> std::fmt::Result {
        if ctx.is_bound.contains(&result) {
            return write!(ctx.out, "v{}", result.index());
        }

        let binding = &ctx.ruleset.bindings[result.index()];

        let mut call =
            |term: TermId,
             parameters: &[BindingId],

             get_sig: fn(&Term, &TypeEnv) -> Option<ExternalSig>| {
                let termdata = &self.termenv.terms[term.index()];
                let sig = get_sig(termdata, self.typeenv).unwrap();
                if let &[ret_ty] = &sig.ret_tys[..] {
                    let (is_ref, _) = self.ty(ret_ty);
                    if is_ref {
                        ctx.set_ref(result, true);
                        write!(ctx.out, "&")?;
                    }
                }
                write!(ctx.out, "{}(ctx", sig.full_name)?;
                debug_assert_eq!(parameters.len(), sig.param_tys.len());
                for (&parameter, &arg_ty) in parameters.iter().zip(sig.param_tys.iter()) {
                    let (is_ref, _) = self.ty(arg_ty);
                    write!(ctx.out, ", ")?;
                    let (before, after) = match (is_ref, ctx.is_ref.contains(&parameter)) {
                        (false, true) => ("", ".clone()"),
                        (true, false) => ("&", ""),
                        _ => ("", ""),
                    };
                    write!(ctx.out, "{}", before)?;
                    self.emit_expr(ctx, parameter)?;
                    write!(ctx.out, "{}", after)?;
                }
                if let ReturnKind::Iterator = sig.ret_kind {
                    write!(ctx.out, ", &mut v{}", result.index())?;
                }
                write!(ctx.out, ")")
            };

        match binding {
            &Binding::ConstInt { val, ty } => self.emit_int(ctx, val, ty),
            Binding::ConstPrim { val } => write!(ctx.out, "{}", &self.typeenv.syms[val.index()]),
            Binding::Argument { index } => write!(ctx.out, "arg{}", index.index()),
            Binding::Extractor { term, parameter } => {
                call(*term, std::slice::from_ref(parameter), Term::extractor_sig)
            }
            Binding::Constructor {
                term, parameters, ..
            } => call(*term, &parameters[..], Term::constructor_sig),

            Binding::MakeVariant {
                ty,
                variant,
                fields,
            } => {
                let (name, variants) = match &self.typeenv.types[ty.index()] {
                    Type::Enum { name, variants, .. } => (name, variants),
                    _ => unreachable!("MakeVariant with primitive type"),
                };
                let variant = &variants[variant.index()];
                write!(
                    ctx.out,
                    "{}::{}",
                    &self.typeenv.syms[name.index()],
                    &self.typeenv.syms[variant.name.index()]
                )?;
                if !fields.is_empty() {
                    ctx.begin_block()?;
                    for (field, value) in variant.fields.iter().zip(fields.iter()) {
                        write!(
                            ctx.out,
                            "{}{}: ",
                            &ctx.indent,
                            &self.typeenv.syms[field.name.index()],
                        )?;
                        self.emit_expr(ctx, *value)?;
                        if ctx.is_ref.contains(value) {
                            write!(ctx.out, ".clone()")?;
                        }
                        writeln!(ctx.out, ",")?;
                    }
                    ctx.end_block_without_newline()?;
                }
                Ok(())
            }

            &Binding::MakeSome { inner } => {
                write!(ctx.out, "Some(")?;
                self.emit_expr(ctx, inner)?;
                write!(ctx.out, ")")
            }
            &Binding::MatchSome { source } => {
                self.emit_expr(ctx, source)?;
                write!(ctx.out, "?")
            }
            &Binding::MatchTuple { source, field } => {
                self.emit_expr(ctx, source)?;
                write!(ctx.out, ".{}", field.index())
            }

            // These are not supposed to happen. If they do, make the generated code fail to compile
            // so this is easier to debug than if we panic during codegen.
            &Binding::MatchVariant { source, field, .. } => {
                self.emit_expr(ctx, source)?;
                write!(ctx.out, ".{} /*FIXME*/", field.index())
            }
            &Binding::Iterator { source } => {
                self.emit_expr(ctx, source)?;
                write!(ctx.out, ".next() /*FIXME*/")
            }
        }
    }

    fn emit_source<W: Write>(
        &self,
        ctx: &mut BodyContext<W>,
        source: BindingId,
        constraint: Constraint,
    ) -> std::fmt::Result {
        if let Constraint::Variant { .. } = constraint {
            if !ctx.is_ref.contains(&source) {
                write!(ctx.out, "&")?;
            }
        }
        self.emit_expr(ctx, source)
    }

    fn emit_constraint<W: Write>(
        &self,
        ctx: &mut BodyContext<W>,
        source: BindingId,
        arm: &MatchArm,
    ) -> std::fmt::Result {
        let MatchArm {
            constraint,
            bindings,
            ..
        } = arm;
        for binding in bindings.iter() {
            if let &Some(binding) = binding {
                ctx.is_bound.insert(binding);
            }
        }
        match *constraint {
            Constraint::ConstInt { val, ty } => self.emit_int(ctx, val, ty),
            Constraint::ConstPrim { val } => {
                write!(ctx.out, "{}", &self.typeenv.syms[val.index()])
            }
            Constraint::Variant { ty, variant, .. } => {
                let (name, variants) = match &self.typeenv.types[ty.index()] {
                    Type::Enum { name, variants, .. } => (name, variants),
                    _ => unreachable!("Variant constraint on primitive type"),
                };
                let variant = &variants[variant.index()];
                write!(
                    ctx.out,
                    "&{}::{}",
                    &self.typeenv.syms[name.index()],
                    &self.typeenv.syms[variant.name.index()]
                )?;
                if !bindings.is_empty() {
                    ctx.begin_block()?;
                    let mut skipped_some = false;
                    for (&binding, field) in bindings.iter().zip(variant.fields.iter()) {
                        if let Some(binding) = binding {
                            write!(
                                ctx.out,
                                "{}{}: ",
                                &ctx.indent,
                                &self.typeenv.syms[field.name.index()]
                            )?;
                            let (is_ref, _) = self.ty(field.ty);
                            if is_ref {
                                ctx.set_ref(binding, true);
                                write!(ctx.out, "ref ")?;
                            }
                            writeln!(ctx.out, "v{},", binding.index())?;
                        } else {
                            skipped_some = true;
                        }
                    }
                    if skipped_some {
                        writeln!(ctx.out, "{}..", &ctx.indent)?;
                    }
                    ctx.end_block_without_newline()?;
                }
                Ok(())
            }
            Constraint::Some => {
                write!(ctx.out, "Some(")?;
                if let Some(binding) = bindings[0] {
                    ctx.set_ref(binding, ctx.is_ref.contains(&source));
                    write!(ctx.out, "v{}", binding.index())?;
                } else {
                    write!(ctx.out, "_")?;
                }
                write!(ctx.out, ")")
            }
        }
    }

    fn emit_int<W: Write>(
        &self,
        ctx: &mut BodyContext<W>,
        val: i128,
        ty: TypeId,
    ) -> Result<(), std::fmt::Error> {
        // For the kinds of situations where we use ISLE, magic numbers are
        // much more likely to be understandable if they're in hex rather than
        // decimal.
        // TODO: use better type info (https://github.com/bytecodealliance/wasmtime/issues/5431)
        if val < 0
            && self.typeenv.types[ty.index()]
                .name(self.typeenv)
                .starts_with('i')
        {
            write!(ctx.out, "-{:#X}", -val)
        } else {
            write!(ctx.out, "{:#X}", val)
        }
    }
}
