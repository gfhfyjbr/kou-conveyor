//! What a patch cannot change under a running app, read from the source.
//!
//! A patch replaces the bodies of functions. Everything else that is written
//! in the workspace decides the shape of what is alive: the fields of a
//! struct, the variants of an enum, a constant an array is as long as, the
//! signature a stored callback is called by. When any of that changes, what
//! the window holds was made by code that laid it out differently, and it has
//! to be taken down and put up again by the new code (see `hot.rs`).
//!
//! [`outline`] folds all of it into one number: every source file of the
//! workspace's crates with the bodies of its functions, its tests and the
//! text of its doc comments left out. Two versions of the source with the
//! same number differ in function bodies only.

use quote::ToTokens as _;
use std::{
    io,
    path::{Path, PathBuf},
};
use syn::visit_mut::{self, VisitMut};

/// The outline of the workspace at `root` (the directory with `crates/`).
pub fn outline(root: &Path) -> io::Result<u64> {
    let mut files = Vec::new();
    for crate_dir in std::fs::read_dir(root.join("crates"))? {
        let source = crate_dir?.path().join("src");
        if source.is_dir() {
            rust_files(&source, &mut files)?;
        }
    }
    files.sort();
    let mut hash = 0xcbf2_9ce4_8422_2325;
    for file in files {
        // The file's place is part of it: a module moved is a module changed.
        let name = file.strip_prefix(root).unwrap_or(&file).to_string_lossy();
        hash = fold(hash, name.as_bytes());
        hash = fold(
            hash,
            outline_of(&std::fs::read_to_string(&file)?).as_bytes(),
        );
    }
    Ok(hash)
}

fn rust_files(directory: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            rust_files(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

fn fold(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// One file's source without what a patch may change: function bodies,
/// tests, and what doc comments say.
pub fn outline_of(source: &str) -> String {
    match syn::parse_file(source) {
        Ok(mut file) => {
            Strip.visit_file_mut(&mut file);
            file.into_token_stream().to_string()
        }
        // What does not parse does not compile either, so no patch comes of
        // it; taken whole, it differs from what it was and from what it will be.
        Err(_) => source.to_owned(),
    }
}

struct Strip;

impl Strip {
    // A body keeps only what it declares: a type declared inside a function
    // is laid out like any other.
    fn body(&mut self, block: &mut syn::Block) {
        block
            .stmts
            .retain(|statement| matches!(statement, syn::Stmt::Item(_)));
        visit_mut::visit_block_mut(self, block);
    }

    fn items(&mut self, items: &mut Vec<syn::Item>) {
        items.retain(|item| !for_tests(item));
    }
}

// Whether an item is compiled for tests only: `#[cfg(test)]` on it.
fn for_tests(item: &syn::Item) -> bool {
    let attributes = match item {
        syn::Item::Mod(item) => &item.attrs,
        syn::Item::Fn(item) => &item.attrs,
        syn::Item::Impl(item) => &item.attrs,
        syn::Item::Use(item) => &item.attrs,
        syn::Item::Struct(item) => &item.attrs,
        syn::Item::Const(item) => &item.attrs,
        _ => return false,
    };
    attributes.iter().any(|attribute| {
        attribute.path().is_ident("cfg")
            && attribute
                .parse_args::<syn::Ident>()
                .is_ok_and(|inside| inside == "test")
    })
}

impl VisitMut for Strip {
    fn visit_file_mut(&mut self, file: &mut syn::File) {
        self.items(&mut file.items);
        visit_mut::visit_file_mut(self, file);
    }

    fn visit_item_mod_mut(&mut self, module: &mut syn::ItemMod) {
        if let Some((_, items)) = &mut module.content {
            self.items(items);
        }
        visit_mut::visit_item_mod_mut(self, module);
    }

    fn visit_item_fn_mut(&mut self, function: &mut syn::ItemFn) {
        self.body(&mut function.block);
        visit_mut::visit_item_fn_mut(self, function);
    }

    fn visit_impl_item_fn_mut(&mut self, function: &mut syn::ImplItemFn) {
        self.body(&mut function.block);
        visit_mut::visit_impl_item_fn_mut(self, function);
    }

    fn visit_trait_item_fn_mut(&mut self, function: &mut syn::TraitItemFn) {
        if let Some(block) = &mut function.default {
            self.body(block);
        }
        visit_mut::visit_trait_item_fn_mut(self, function);
    }

    fn visit_attribute_mut(&mut self, attribute: &mut syn::Attribute) {
        // A doc comment is an attribute with its text in it.
        if attribute.path().is_ident("doc") {
            attribute.meta = syn::parse_quote!(doc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEFORE: &str = r#"
        /// A pane.
        pub struct Pane { id: String, title: String }
        const ROWS: usize = 24;
        impl Pane {
            /// Its title.
            pub fn title(&self) -> &str { &self.title }
        }
        fn helper(count: usize) -> usize { count + 1 }
        #[cfg(test)]
        mod tests { #[test] fn it_works() { assert_eq!(super::helper(1), 2); } }
    "#;

    #[test]
    fn bodies_tests_and_doc_text_are_not_part_of_the_outline() {
        // Another body, another doc comment, another test, other spacing.
        let after = r#"
            /// A pane of the window.
            pub struct Pane { id: String, title: String }
            const ROWS: usize = 24;
            impl Pane {
                /// What it is called.
                pub fn title(&self) -> &str { let title = &self.title; title }
            }
            fn helper(count: usize) -> usize {
                let more = count + 1;
                more
            }
            #[cfg(test)]
            mod tests { #[test] fn it_still_works() { assert_eq!(super::helper(2), 3); } }
        "#;
        assert_eq!(outline_of(BEFORE), outline_of(after));
    }

    #[test]
    fn whatever_shapes_what_is_alive_is_part_of_the_outline() {
        let outline = outline_of(BEFORE);
        for (what, changed) in [
            (
                "a field added",
                BEFORE.replace("title: String }", "title: String, seen: bool }"),
            ),
            ("a field's type", BEFORE.replace("id: String", "id: u64")),
            ("a constant", BEFORE.replace("= 24", "= 25")),
            (
                "a signature",
                BEFORE.replace("fn helper(count: usize)", "fn helper(count: u32)"),
            ),
            (
                "a type declared in a body",
                BEFORE.replace("{ count + 1 }", "{ struct Step(u8); count + 1 }"),
            ),
            (
                "a derive",
                BEFORE.replace("pub struct Pane", "#[derive(Clone)] pub struct Pane"),
            ),
        ] {
            assert_ne!(outline, outline_of(&changed), "{what}");
        }
        // Source that does not parse is not like anything that does.
        assert_ne!(outline, outline_of("pub struct {"));
    }

    #[test]
    fn the_workspace_has_an_outline_that_holds_still() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let first = outline(&root).expect("the sources are there");
        assert_eq!(outline(&root).unwrap(), first);
        // A workspace in another place, with one file changed in its shape.
        let copy = tempfile::tempdir().unwrap();
        let source = copy.path().join("crates/one/src");
        std::fs::create_dir_all(source.join("deeper")).unwrap();
        std::fs::write(source.join("lib.rs"), BEFORE).unwrap();
        std::fs::write(source.join("deeper/more.rs"), "pub fn more() {}").unwrap();
        std::fs::write(copy.path().join("crates/not-a-crate.txt"), "x").unwrap();
        let before = outline(copy.path()).unwrap();
        std::fs::write(
            source.join("deeper/more.rs"),
            "pub fn more() { let _ = 1; }",
        )
        .unwrap();
        assert_eq!(outline(copy.path()).unwrap(), before, "a body");
        std::fs::write(source.join("deeper/more.rs"), "pub fn more(_: u8) {}").unwrap();
        assert_ne!(outline(copy.path()).unwrap(), before, "a signature");
    }
}
