use std::{
  env, fs,
  ops::Range,
  path::{Path, PathBuf},
};

use proc_macro2::{Span, TokenStream, TokenTree};
use syn::{
  Expr, ExprLit, File, Item, ItemConst, Lit, LitStr, UseTree, parse::Parser, punctuated::Punctuated,
  spanned::Spanned, token::Comma, visit::Visit,
};

struct Edit {
  range: Range<usize>,
  replacement: String,
}

struct MuddyRewriter<'a> {
  source: &'a str,
  source_offset: usize,
  in_const: bool,
  edits: Vec<Edit>,
}

impl<'a> MuddyRewriter<'a> {
  fn new(source: &'a str, file: &File) -> Self {
    let source_offset = if source.starts_with('\u{feff}') { 3 } else { 0 }
      + file.shebang.as_ref().map_or(0, String::len);
    Self { source, source_offset, in_const: false, edits: Vec::new() }
  }

  fn range(&self, span: Span) -> Range<usize> {
    let range = span.byte_range();
    range.start + self.source_offset..range.end + self.source_offset
  }

  fn wrap(&mut self, literal: &LitStr) {
    if literal.value() == "C" {
      return;
    }
    let range = self.range(literal.span());
    let replacement = format!("muddy!({})", &self.source[range.clone()]);
    self.edits.push(Edit { range, replacement });
  }

  fn has_muddy_import(tree: &UseTree) -> bool {
    match tree {
      UseTree::Path(path) if path.ident == "muddy" => match path.tree.as_ref() {
        UseTree::Name(name) => name.ident == "muddy",
        UseTree::Group(group) => group.items.iter().any(|item| matches!(item, UseTree::Name(name) if name.ident == "muddy")),
        UseTree::Glob(_) => true,
        _ => false,
      },
      UseTree::Path(path) => Self::has_muddy_import(&path.tree),
      UseTree::Group(group) => group.items.iter().any(Self::has_muddy_import),
      _ => false,
    }
  }

  fn visit_macro_blocks(&mut self, tokens: TokenStream) {
    for token in tokens {
      if let TokenTree::Group(group) = token {
        if group.delimiter() == proc_macro2::Delimiter::Brace {
          if let Ok(block) = syn::parse2::<syn::Block>(TokenStream::from(TokenTree::Group(group.clone()))) {
            self.visit_block(&block);
            continue;
          }
        }
        self.visit_macro_blocks(group.stream());
      }
    }
  }

  fn finish(mut self, file: &File) -> Result<String, Box<dyn std::error::Error>> {
    if self.edits.is_empty() {
      return Ok(self.source.to_owned());
    }

    let has_import = file.items.iter().any(|item| {
      matches!(item, Item::Use(item_use) if Self::has_muddy_import(&item_use.tree))
    });
    if !has_import {
      let mut position = self.range(file.items[0].span()).start;
      let line_start = self.source[..position].rfind('\n').map_or(self.source_offset, |index| index + 1);
      if self.source[line_start..position].chars().all(char::is_whitespace) {
        position = line_start;
      }
      let newline = if self.source.contains("\r\n") { "\r\n" } else { "\n" };
      self.edits.push(Edit {
        range: position..position,
        replacement: format!("use muddy::muddy;{newline}"),
      });
    }

    self.edits.sort_by_key(|edit| edit.range.start);
    let mut output = String::new();
    let mut cursor = 0;
    for edit in self.edits {
      if edit.range.start < cursor || self.source.get(edit.range.clone()).is_none() {
        return Err("overlapping or invalid source edits".into());
      }
      output.push_str(&self.source[cursor..edit.range.start]);
      output.push_str(&edit.replacement);
      cursor = edit.range.end;
    }
    output.push_str(&self.source[cursor..]);
    Ok(output)
  }
}

impl<'ast> Visit<'ast> for MuddyRewriter<'_> {
  fn visit_expr(&mut self, node: &'ast Expr) {
    if !self.in_const {
      if let Expr::Lit(ExprLit { lit: Lit::Str(literal), .. }) = node {
        self.wrap(literal);
        return;
      }
    }
    syn::visit::visit_expr(self, node);
  }

  fn visit_item_const(&mut self, item: &'ast ItemConst) {
    let previous = self.in_const;
    self.in_const = true;
    syn::visit::visit_item_const(self, item);
    self.in_const = previous;
  }

  fn visit_attribute(&mut self, _: &'ast syn::Attribute) {
  }

  fn visit_macro(&mut self, mac: &'ast syn::Macro) {
    if self.in_const {
      return;
    }

    if let Some(ident) = mac.path.segments.last().map(|segment| &segment.ident) {
      let name = ident.to_string();
      if name == "muddy" {
        return;
      }

      let format_index = match name.as_str() {
        "println" | "print" | "eprintln" | "eprint" | "format" | "format_args" => Some(0),
        "write" | "writeln" => Some(1),
        _ => None,
      };

      if format_index.is_some() || name == "dbg" {
        if let Ok(args) = Punctuated::<Expr, Comma>::parse_terminated.parse2(mac.tokens.clone()) {
          for (index, arg) in args.iter().enumerate() {
            if Some(index) == format_index {
              if let Expr::Lit(ExprLit { lit: Lit::Str(literal), .. }) = arg {
                let value = literal.value();
                let split = value.find('{').unwrap_or(value.len());
                let prefix = &value[..split];
                if !prefix.is_empty() && value != "C" {
                  let format = LitStr::new(&format!("{{}}{}", &value[split..]), literal.span());
                  let prefix = LitStr::new(&prefix.replace("}}", "}"), literal.span());
                  self.edits.push(Edit {
                    range: self.range(literal.span()),
                    replacement: format!("{}, muddy!({})", format.token(), prefix.token()),
                  });
                }
              }
            } else {
              self.visit_expr(arg);
            }
          }
          return;
        }
      }
    }

    self.visit_macro_blocks(mac.tokens.clone());
  }
}

fn rewrite(source: &str) -> Result<String, Box<dyn std::error::Error>> {
  let file = syn::parse_file(source)?;
  let mut rewriter = MuddyRewriter::new(source, &file);
  rewriter.visit_file(&file);
  rewriter.finish(&file)
}

fn process_file(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
  println!("Processing: {}", path.display());
  let source = fs::read_to_string(path)?;
  let output = rewrite(&source)?;
  if output != source {
    fs::write(path, output)?;
  }
  Ok(())
}

fn process_directory(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
  for entry in fs::read_dir(dir)? {
    let path = entry?.path();
    if path.is_dir() {
      process_directory(&path)?;
    } else if path.extension().is_some_and(|ext| ext == "rs") {
      process_file(&path)?;
    }
  }
  Ok(())
}

fn main() {
  let path = env::args().nth(1).expect("Usage: muddy-wrap <file.rs|directory>");
  let path = PathBuf::from(path);
  let result = if path.is_file() {
    process_file(&path)
  } else if path.is_dir() {
    process_directory(&path)
  } else {
    Err("path does not exist or is not a file/directory".into())
  };
  if let Err(error) = result {
    eprintln!("Error processing {}: {error}", path.display());
    std::process::exit(1);
  }
}

#[cfg(test)]
mod tests {
  use super::rewrite;

  fn check(source: &str, expected: &str) {
    let output = rewrite(source).unwrap();
    assert_eq!(output, expected);
    syn::parse_file(&output).unwrap();
    assert_eq!(rewrite(&output).unwrap(), output, "second pass must not change the output");
  }

  #[test]
  fn preserves_comments_spacing_and_raw_literals() {
    check(
      "// heading\nuse muddy::muddy;\n\nfn main( ) {\n\t// keep me\n\tlet café  =  r##\"héllo\"##;  /* trailing */\n}\n",
      "// heading\nuse muddy::muddy;\n\nfn main( ) {\n\t// keep me\n\tlet café  =  muddy!(r##\"héllo\"##);  /* trailing */\n}\n",
    );
  }

  #[test]
  fn inserts_import_after_inner_attributes_before_item_docs() {
    check(
      "// heading\n//! Module documentation\n#![allow(unused)]\n\n/// Function documentation\n#[inline]\nfn main() { let x = \"hello\"; }\n",
      "// heading\n//! Module documentation\n#![allow(unused)]\n\nuse muddy::muddy;\n/// Function documentation\n#[inline]\nfn main() { let x = muddy!(\"hello\"); }\n",
    );
  }

  #[test]
  fn preserves_bom_shebang_crlf_and_missing_final_newline() {
    check(
      "\u{feff}#!/usr/bin/env rust-script\r\n// heading\r\nfn main( ) {\r\n\tlet x = \"你好\";\r\n}",
      "\u{feff}#!/usr/bin/env rust-script\r\n// heading\r\nuse muddy::muddy;\r\nfn main( ) {\r\n\tlet x = muddy!(\"你好\");\r\n}",
    );
  }

  #[test]
  fn leaves_files_without_transformations_untouched() {
    for source in [
      "// comments only\n",
      "#!/usr/bin/env rust-script",
      "#![doc = \"documentation\"]\nconst VALUE: &str = \"constant\";\n",
      "extern \"C\" { fn example(); }\nfn main() { let abi = \"C\"; }\n",
    ] {
      check(source, source);
    }
  }

  #[test]
  fn preserves_multiline_format_arguments_and_comments() {
    check(
      "use muddy::muddy;\nfn main() {\n  println!(\n    \"Hello {}\", /* before arg */\n    \"world\", // after arg\n  );\n}\n",
      "use muddy::muddy;\nfn main() {\n  println!(\n    \"{}{}\", muddy!(\"Hello \"), /* before arg */\n    muddy!(\"world\"), // after arg\n  );\n}\n",
    );
  }

  #[test]
  fn visits_blocks_inside_unknown_macros() {
    check(
      "use muddy::muddy;\nfn main() { custom! { { // keep\n  let x=\"nested\";\n} } }\n",
      "use muddy::muddy;\nfn main() { custom! { { // keep\n  let x=muddy!(\"nested\");\n} } }\n",
    );
  }

  #[test]
  fn handles_writer_destination_and_debug_expressions() {
    check(
      "use muddy::muddy;\nfn main() { write!(buffer, \"Hello {}\", \"world\"); dbg!(\"debug\"); }\n",
      "use muddy::muddy;\nfn main() { write!(buffer, \"{}{}\", muddy!(\"Hello \"), muddy!(\"world\")); dbg!(muddy!(\"debug\")); }\n",
    );
  }

  #[test]
  fn preserves_existing_group_imports_and_nested_const_contexts() {
    check(
      "use muddy::{muddy};\nconst VALUE: &str = { const INNER: &str = \"inner\"; \"outer\" };\nfn main() { let x = \"hello\"; }\n",
      "use muddy::{muddy};\nconst VALUE: &str = { const INNER: &str = \"inner\"; \"outer\" };\nfn main() { let x = muddy!(\"hello\"); }\n",
    );
  }

  #[test]
  fn preserves_multiline_unicode_literal_spans() {
    check(
      "use muddy::muddy;\nfn main() { let café = \"héllo\n你好\"; let next = \"next\"; }\n",
      "use muddy::muddy;\nfn main() { let café = muddy!(\"héllo\n你好\"); let next = muddy!(\"next\"); }\n",
    );
  }

  #[test]
  fn keeps_leading_indentation_when_inserting_import() {
    check(
      "  fn main() { let x = \"hello\"; }\n",
      "use muddy::muddy;\n  fn main() { let x = muddy!(\"hello\"); }\n",
    );
  }
}
