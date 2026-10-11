//! MiniJinja syntax analysis shared by config migration and error diagnostics.
//!
//! The dependency parser owns grammar and source spans. This exhaustive visitor
//! records reads, bindings, and unambiguous output expressions; it does not
//! reconstruct runtime scopes or guess which operand produced an undefined value.
//! With macros and multi-template loading disabled, every AST variant is handled.
//! Dependency AST changes fail compilation rather than silently skipping syntax.

use std::collections::HashSet;
use std::ops::Range;

use minijinja::machinery::{ast, parse as parse_template};

pub(super) struct TemplateVars<'a> {
    /// The name and byte range of every `Expr::Var`, in visit order.
    pub(super) reads: Vec<(&'a str, Range<usize>)>,
    /// Locally bound names are excluded conservatively across the template.
    /// Migration checks both retired and replacement names: renaming a global
    /// to a name bound by a loop or assignment would capture the local value.
    /// Diagnostics similarly avoid assigning a missing local to a global.
    pub(super) bound: HashSet<&'a str>,
    outputs: Vec<(&'a str, Range<usize>)>,
}

impl<'a> TemplateVars<'a> {
    /// `None` when MiniJinja can't parse `template` — the templates its
    /// renderer rejects too, left untouched rather than guessed at.
    pub(super) fn of(template: &'a str) -> Option<Self> {
        // Whitespace settings shape literal output, never variable spans.
        let ast =
            parse_template(template, "<config>", Default::default(), Default::default()).ok()?;
        let mut vars = TemplateVars {
            reads: Vec::new(),
            bound: HashSet::new(),
            outputs: Vec::new(),
        };
        vars.stmt(&ast);
        Some(vars)
    }

    /// Name an output only when the engine range selects a plain global read,
    /// optionally passed through argument-free filters. Locals, lookups and
    /// compound expressions cannot establish the missing input this way.
    pub(super) fn output_at(&self, range: Range<usize>) -> Option<&'a str> {
        self.outputs.iter().find_map(|(name, span)| {
            (span.start <= range.start && span.end >= range.end && !self.bound.contains(name))
                .then_some(*name)
        })
    }

    fn body(&mut self, body: &[ast::Stmt<'a>]) {
        for stmt in body {
            self.stmt(stmt);
        }
    }

    fn stmt(&mut self, stmt: &ast::Stmt<'a>) {
        match stmt {
            ast::Stmt::Template(node) => self.body(&node.children),
            ast::Stmt::EmitExpr(node) => {
                let mut expr = &node.expr;
                while let ast::Expr::Filter(filter) = expr {
                    if !filter.args.is_empty() {
                        break;
                    }
                    let Some(input) = &filter.expr else {
                        break;
                    };
                    expr = input;
                }
                if let ast::Expr::Var(var) = expr {
                    let span = node.span();
                    self.outputs
                        .push((var.id, span.start_offset as usize..span.end_offset as usize));
                }
                self.expr(&node.expr);
            }
            // Literal output — the text around the tags, and everything inside
            // a `{% raw %}` block.
            ast::Stmt::EmitRaw(_) => {}
            ast::Stmt::ForLoop(node) => {
                self.bound.insert("loop");
                self.target(&node.target);
                self.expr(&node.iter);
                if let Some(filter) = &node.filter_expr {
                    self.expr(filter);
                }
                self.body(&node.body);
                self.body(&node.else_body);
            }
            ast::Stmt::IfCond(node) => {
                self.expr(&node.expr);
                self.body(&node.true_body);
                self.body(&node.false_body);
            }
            ast::Stmt::WithBlock(node) => {
                for (target, value) in &node.assignments {
                    self.target(target);
                    self.expr(value);
                }
                self.body(&node.body);
            }
            ast::Stmt::Set(node) => {
                self.target(&node.target);
                self.expr(&node.expr);
            }
            ast::Stmt::SetBlock(node) => {
                self.target(&node.target);
                if let Some(filter) = &node.filter {
                    self.expr(filter);
                }
                self.body(&node.body);
            }
            ast::Stmt::AutoEscape(node) => {
                self.expr(&node.enabled);
                self.body(&node.body);
            }
            ast::Stmt::FilterBlock(node) => {
                self.expr(&node.filter);
                self.body(&node.body);
            }
            ast::Stmt::Do(node) => self.call(&node.call),
        }
    }

    fn expr(&mut self, expr: &ast::Expr<'a>) {
        match expr {
            ast::Expr::Var(node) => {
                let span = node.span();
                self.reads.push((
                    node.id,
                    span.start_offset as usize..span.end_offset as usize,
                ));
            }
            ast::Expr::Const(_) => {}
            ast::Expr::Slice(node) => {
                self.expr(&node.expr);
                for bound in [&node.start, &node.stop, &node.step].into_iter().flatten() {
                    self.expr(bound);
                }
            }
            ast::Expr::UnaryOp(node) => self.expr(&node.expr),
            ast::Expr::BinOp(node) => {
                self.expr(&node.left);
                self.expr(&node.right);
            }
            ast::Expr::Compare(node) => {
                self.expr(&node.expr);
                for op in &node.ops {
                    self.expr(&op.expr);
                }
            }
            ast::Expr::IfExpr(node) => {
                self.expr(&node.test_expr);
                self.expr(&node.true_expr);
                if let Some(false_expr) = &node.false_expr {
                    self.expr(false_expr);
                }
            }
            // A filter or test names a function the environment supplies, not
            // a variable, so only its input and arguments are reads.
            ast::Expr::Filter(node) => {
                if let Some(expr) = &node.expr {
                    self.expr(expr);
                }
                self.args(&node.args);
            }
            ast::Expr::Test(node) => {
                self.expr(&node.expr);
                self.args(&node.args);
            }
            // `{{ foo.repo_root }}` reads `foo`; the attribute belongs to
            // whatever that resolves to, never to the deprecated global.
            ast::Expr::GetAttr(node) => self.expr(&node.expr),
            ast::Expr::GetItem(node) => {
                self.expr(&node.expr);
                self.expr(&node.subscript_expr);
            }
            ast::Expr::Call(node) => self.call(node),
            ast::Expr::List(node) => {
                for item in &node.items {
                    self.expr(item);
                }
            }
            ast::Expr::Map(node) => {
                for entry in node.keys.iter().chain(&node.values) {
                    self.expr(entry);
                }
            }
        }
    }

    fn call(&mut self, call: &ast::Call<'a>) {
        self.expr(&call.expr);
        self.args(&call.args);
    }

    /// A keyword argument's name belongs to the call it is passed to, so only
    /// the argument values are reads.
    fn args(&mut self, args: &[ast::CallArg<'a>]) {
        for arg in args {
            match arg {
                ast::CallArg::Pos(expr)
                | ast::CallArg::Kwarg(_, expr)
                | ast::CallArg::PosSplat(expr)
                | ast::CallArg::KwargSplat(expr) => self.expr(expr),
            }
        }
    }

    /// The names an assignment target binds.
    fn target(&mut self, target: &ast::Expr<'a>) {
        match target {
            ast::Expr::Var(node) => {
                self.bound.insert(node.id);
            }
            // A tuple target, nested arbitrarily: `{% for (a, (b, c)) in … %}`.
            ast::Expr::List(node) => {
                for item in &node.items {
                    self.target(item);
                }
            }
            // A dotted target mutates an attribute of whatever the path
            // resolves to, so `{% set repo_root.x = … %}` reads the global
            // rather than binding it.
            read => self.expr(read),
        }
    }
}
