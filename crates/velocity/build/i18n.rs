//! Collect literal app-owned UI keys for the round-trip catalog test.
use std::{collections::BTreeSet, fs, io, path::Path};
use syn::{
    Expr, Lit,
    visit::{self, Visit},
};

pub(super) fn generate(out: &Path) -> io::Result<()> {
    let packages = Path::new("resources/Packages");
    println!("cargo:rerun-if-changed={}", packages.display());
    let mut catalogs = Vec::new();
    for package in fs::read_dir(packages)? {
        let dir = package?.path().join("Localization");
        if !dir.is_dir() {
            continue;
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext == "sublime-localization")
            {
                let relative = path.strip_prefix(packages).expect("package path");
                catalogs.push(format!(
                    "({:?}, include_bytes!({:?}) as &[u8]),\n",
                    format!("Packages/{}", vel_config::paths::slash_separated(relative)),
                    path.canonicalize()?
                ));
            }
        }
    }
    catalogs.sort_unstable();
    fs::write(
        out.join("localizations.rs"),
        format!("&[{}]", catalogs.concat()),
    )?;

    println!("cargo:rerun-if-changed=src");
    let mut keys = Keys::default();
    walk(Path::new("src"), &mut keys)?;
    fs::write(
        out.join("ui_keys.rs"),
        format!("&{:?}", keys.0.into_iter().collect::<Vec<_>>()),
    )
}

fn walk(dir: &Path, keys: &mut Keys) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            walk(&path, keys)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let ast = syn::parse_file(&fs::read_to_string(path)?).map_err(io::Error::other)?;
            keys.visit_file(&ast);
        }
    }
    Ok(())
}

#[derive(Default)]
struct Keys(BTreeSet<String>);
impl Keys {
    fn record(&mut self, key: Option<&Expr>) {
        match key {
            Some(Expr::Lit(lit)) => {
                if let Lit::Str(key) = &lit.lit {
                    self.0.insert(key.value());
                }
            }
            Some(Expr::If(branch)) => {
                if let Some(syn::Stmt::Expr(value, None)) = branch.then_branch.stmts.last() {
                    self.record(Some(value));
                }
                if let Some((_, otherwise)) = &branch.else_branch {
                    self.record(Some(otherwise));
                }
            }
            Some(Expr::Match(matched)) => {
                // `tr(match reason { .. => "…", })` picks one literal; every arm is a key.
                for arm in &matched.arms {
                    if arm.guard.is_none() {
                        self.record(Some(&arm.body));
                    }
                }
            }
            Some(Expr::Block(block)) => {
                if let Some(syn::Stmt::Expr(value, None)) = block.block.stmts.last() {
                    self.record(Some(value));
                }
            }
            _ => {}
        }
    }
}
impl<'ast> Visit<'ast> for Keys {
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if matches!(
            node.method.to_string().as_str(),
            "tr" | "tr_fmt" | "caption" | "palette_caption"
        ) {
            self.record(node.args.first());
        }
        visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let Expr::Path(path) = node.func.as_ref()
            && let Some(segment) = path.path.segments.last()
            && matches!(
                segment.ident.to_string().as_str(),
                "tr" | "tr_fmt" | "compose"
            )
        {
            self.record(node.args.first());
        }
        visit::visit_expr_call(self, node);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        // Common expression-list macros retain calls, e.g. vec![tr.tr("…")].
        // Other token DSLs (json!, matches!, etc.) aren't Rust expression lists.
        if (node.path.is_ident("vec") || node.path.is_ident("format"))
            && let Ok(args) = node.parse_body_with(
                syn::punctuated::Punctuated::<Expr, syn::Token![,]>::parse_terminated,
            )
        {
            for arg in &args {
                self.visit_expr(arg);
            }
        }
    }
}
