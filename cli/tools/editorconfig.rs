// Copyright 2018-2026 the Deno authors. MIT license.

//! Support for inferring `deno fmt` options from `.editorconfig` files.
//!
//! When `use_tabs`, `indent_width`, or `line_width` are not explicitly set in
//! the Deno configuration file or via CLI flags, Deno will search for an
//! `.editorconfig` file and use its `indent_style`, `indent_size`/`tab_width`,
//! and `max_line_length` properties as defaults.
//!
//! See <https://editorconfig.org/> for the `.editorconfig` specification.

use std::path::Path;
use std::path::PathBuf;

use crate::args::FmtOptionsConfig;

/// Applies `.editorconfig` settings to the given `FmtOptionsConfig` for any
/// options that are not already set (`None`).
///
/// It searches for `.editorconfig` files starting from `dir` and walking up
/// the directory tree until a file with `root = true` is found or the
/// filesystem root is reached. Properties from files closer to `dir` take
/// precedence over those from files higher up.
pub fn maybe_apply_editorconfig(dir: &Path, options: &mut FmtOptionsConfig) {
  if options.use_tabs.is_some()
    && options.indent_width.is_some()
    && options.line_width.is_some()
  {
    return; // All relevant options already set — nothing to do
  }

  let ec = load_editorconfig(dir);

  if options.use_tabs.is_none() {
    options.use_tabs = ec.use_tabs;
  }
  if options.indent_width.is_none() {
    options.indent_width = ec.indent_width;
  }
  if options.line_width.is_none() {
    options.line_width = ec.line_width;
  }
}

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct EditorConfigProps {
  use_tabs: Option<bool>,
  indent_width: Option<u8>,
  line_width: Option<u32>,
}

// ---------------------------------------------------------------------------
// File discovery & parsing
// ---------------------------------------------------------------------------

/// Walk up from `dir` collecting `.editorconfig` files.  Returns combined
/// properties after applying precedence rules (inner file wins over outer).
fn load_editorconfig(dir: &Path) -> EditorConfigProps {
  // Collect (editorconfig_dir, file_contents) from `dir` upward.
  let mut ec_files: Vec<(PathBuf, String)> = Vec::new();
  let mut current = dir;

  loop {
    let ec_path = current.join(".editorconfig");
    if let Ok(content) = std::fs::read_to_string(&ec_path) {
      let is_root = parse_is_root(&content);
      ec_files.push((current.to_path_buf(), content));
      if is_root {
        break;
      }
    }
    match current.parent() {
      Some(parent) => current = parent,
      None => break,
    }
  }

  // Reverse so we process outermost file first; inner values overwrite outer.
  ec_files.reverse();

  let mut props = EditorConfigProps::default();
  for (ec_dir, content) in &ec_files {
    parse_editorconfig(content, ec_dir, dir, &mut props);
  }
  props
}

/// Returns `true` when the top-level (pre-section) content of the file
/// contains `root = true`.
fn parse_is_root(content: &str) -> bool {
  for line in content.lines() {
    let line = line.trim();
    if line.is_empty() || is_comment(line) {
      continue;
    }
    if line.starts_with('[') {
      break; // reached the first section — stop
    }
    if let Some((key, val)) = parse_key_value(line)
      && key == "root"
      && val == "true"
    {
      return true;
    }
  }
  false
}

/// Parse one `.editorconfig` file and accumulate properties that match the
/// `target_dir`.  Sections matched against `target_dir/main.ts` as a
/// representative source file (this covers `[*]`, `[*.ts]`, `[*.{ts,js}]`,
/// etc.).
fn parse_editorconfig(
  content: &str,
  ec_dir: &Path,
  target_dir: &Path,
  props: &mut EditorConfigProps,
) {
  // Representative file used for section-pattern matching.
  let representative = target_dir.join("main.ts");

  let mut in_matching_section = false;
  let mut section_props = EditorConfigProps::default();

  for line in content.lines() {
    let line = line.trim();

    if line.is_empty() || is_comment(line) {
      continue;
    }

    if line.starts_with('[') {
      // Commit any previous section's results.
      if in_matching_section {
        merge_props(&section_props, props);
      }
      section_props = EditorConfigProps::default();

      in_matching_section = if let Some(pattern) =
        line.strip_prefix('[').and_then(|s| s.strip_suffix(']'))
      {
        section_matches(pattern, ec_dir, &representative)
      } else {
        false
      };
      continue;
    }

    if in_matching_section
      && let Some((key, val)) = parse_key_value(line)
    {
      match key.as_str() {
        "indent_style" => match val.as_str() {
          "tab" => section_props.use_tabs = Some(true),
          "space" => section_props.use_tabs = Some(false),
          _ => {}
        },
        "indent_size" | "tab_width" => {
          // "tab_width" is only meaningful alongside "indent_style = tab",
          // but we still record it so callers can decide.
          if val != "unset" && let Ok(n) = val.parse::<u8>() {
            section_props.indent_width = Some(n);
          }
        }
        "max_line_length" => {
          if val != "off" && val != "unset" && let Ok(n) = val.parse::<u32>() {
            section_props.line_width = Some(n);
          }
        }
        _ => {}
      }
    }
  }

  // Commit the last section.
  if in_matching_section {
    merge_props(&section_props, props);
  }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_comment(line: &str) -> bool {
  line.starts_with('#') || line.starts_with(';')
}

fn parse_key_value(line: &str) -> Option<(String, String)> {
  let eq = line.find('=')?;
  let key = line[..eq].trim().to_lowercase();
  let val = line[eq + 1..].trim().to_lowercase();
  if key.is_empty() || val.is_empty() {
    return None;
  }
  Some((key, val))
}

/// Overwrite `dst` fields with any `Some` value from `src`.
fn merge_props(src: &EditorConfigProps, dst: &mut EditorConfigProps) {
  if let Some(v) = src.use_tabs {
    dst.use_tabs = Some(v);
  }
  if let Some(v) = src.indent_width {
    dst.indent_width = Some(v);
  }
  if let Some(v) = src.line_width {
    dst.line_width = Some(v);
  }
}

/// Returns `true` when the editorconfig glob `pattern` (relative to
/// `ec_dir`) matches the `file` path.
fn section_matches(pattern: &str, ec_dir: &Path, file: &Path) -> bool {
  // Build the full pattern by prepending the editorconfig directory when the
  // pattern does not start with `/` (i.e. it is relative).
  let full_pattern: PathBuf = if let Some(stripped) = pattern.strip_prefix('/')
  {
    ec_dir.join(stripped)
  } else {
    ec_dir.join(pattern)
  };

  editorconfig_glob_match(
    &full_pattern.to_string_lossy(),
    &file.to_string_lossy(),
  )
}

/// Minimal editorconfig-compatible glob matcher.
///
/// Rules (from <https://editorconfig.org/#file-format-details>):
/// * `*`  — matches any character string, but NOT path separators
/// * `**` — matches any character string, INCLUDING path separators
/// * `?`  — matches any single character (not a path separator)
/// * `{s1,s2,...}` — matches any of the strings `s1`, `s2`, …
/// * `[seq]` / `[!seq]` — character classes (best-effort)
///
/// Matching is case-sensitive on all platforms to stay consistent with the
/// editorconfig spec.
fn editorconfig_glob_match(pattern: &str, path: &str) -> bool {
  glob_match_impl(
    pattern.as_bytes(),
    path.as_bytes(),
    std::path::MAIN_SEPARATOR as u8,
  )
}

fn glob_match_impl(pattern: &[u8], path: &[u8], sep: u8) -> bool {
  let mut pi = 0usize; // index into pattern
  let mut si = 0usize; // index into path

  while pi < pattern.len() {
    match pattern[pi] {
      b'*' => {
        // Check for `**`
        if pi + 1 < pattern.len() && pattern[pi + 1] == b'*' {
          // `**` — skip any number of characters including separators
          pi += 2;
          // Consume optional trailing separator in pattern
          if pi < pattern.len() && pattern[pi] == sep {
            pi += 1;
          }
          if pi == pattern.len() {
            return true; // `**` at end matches everything
          }
          // Try matching the rest of the pattern from every position in path
          for i in si..=path.len() {
            if glob_match_impl(&pattern[pi..], &path[i..], sep) {
              return true;
            }
          }
          return false;
        } else {
          // Single `*` — match any characters except `sep`
          pi += 1;
          if pi == pattern.len() {
            // `*` at end: no separator allowed in remaining path
            return !path[si..].contains(&sep);
          }
          // Try consuming zero or more non-separator characters
          while si <= path.len() {
            if glob_match_impl(&pattern[pi..], &path[si..], sep) {
              return true;
            }
            if si < path.len() && path[si] != sep {
              si += 1;
            } else {
              break;
            }
          }
          return false;
        }
      }
      b'?' => {
        if si >= path.len() || path[si] == sep {
          return false;
        }
        pi += 1;
        si += 1;
      }
      b'{' => {
        // Find matching `}`
        let end = match find_matching_brace(pattern, pi) {
          Some(e) => e,
          None => {
            // Treat as literal
            if si >= path.len() || path[si] != pattern[pi] {
              return false;
            }
            pi += 1;
            si += 1;
            continue;
          }
        };
        let alternatives = pattern[pi + 1..end].split(|&b| b == b',');
        for alt in alternatives {
          let mut combined =
            Vec::with_capacity(alt.len() + pattern[end + 1..].len());
          combined.extend_from_slice(alt);
          combined.extend_from_slice(&pattern[end + 1..]);
          if glob_match_impl(&combined, &path[si..], sep) {
            return true;
          }
        }
        return false;
      }
      b'[' => {
        // Character class — find closing `]`
        let class_end = match pattern[pi + 1..].iter().position(|&b| b == b']') {
          Some(pos) => pi + 1 + pos,
          None => {
            // Malformed; treat `[` as literal
            if si >= path.len() || path[si] != b'[' {
              return false;
            }
            pi += 1;
            si += 1;
            continue;
          }
        };
        if si >= path.len() {
          return false;
        }
        let ch = path[si];
        let class_content = &pattern[pi + 1..class_end];
        let negate = class_content.first() == Some(&b'!');
        let chars = if negate { &class_content[1..] } else { class_content };
        let matched = chars.contains(&ch);
        let result = if negate { !matched } else { matched };
        if !result {
          return false;
        }
        pi = class_end + 1;
        si += 1;
      }
      literal => {
        if si >= path.len() || path[si] != literal {
          return false;
        }
        pi += 1;
        si += 1;
      }
    }
  }

  si == path.len()
}

/// Find the index of the `}` that closes the `{` at `pattern[start]`.
fn find_matching_brace(pattern: &[u8], start: usize) -> Option<usize> {
  let mut depth = 0usize;
  for (i, &b) in pattern[start..].iter().enumerate() {
    match b {
      b'{' => depth += 1,
      b'}' => {
        depth -= 1;
        if depth == 0 {
          return Some(start + i);
        }
      }
      _ => {}
    }
  }
  None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_glob_match_star() {
    assert!(editorconfig_glob_match("*.ts", "foo.ts"));
    assert!(!editorconfig_glob_match("*.ts", "foo/bar.ts"));
    assert!(editorconfig_glob_match("*.{ts,js}", "foo.ts"));
    assert!(editorconfig_glob_match("*.{ts,js}", "foo.js"));
    assert!(!editorconfig_glob_match("*.{ts,js}", "foo.rs"));
  }

  #[test]
  fn test_glob_match_double_star() {
    assert!(editorconfig_glob_match("**/*.ts", "foo/bar.ts"));
    assert!(editorconfig_glob_match("**/*.ts", "foo/bar/baz.ts"));
    assert!(editorconfig_glob_match("**", "foo/bar.ts"));
  }

  #[test]
  fn test_glob_match_question() {
    assert!(editorconfig_glob_match("fo?.ts", "foo.ts"));
    assert!(!editorconfig_glob_match("fo?.ts", "fo.ts"));
  }

  #[test]
  fn test_parse_is_root() {
    assert!(parse_is_root("root = true\n[*]\nindent_style = space\n"));
    assert!(!parse_is_root("[*]\nroot = true\n"));
    assert!(!parse_is_root("root = false\n"));
  }

  #[test]
  fn test_parse_key_value() {
    assert_eq!(
      parse_key_value("indent_style = space"),
      Some(("indent_style".into(), "space".into()))
    );
    assert_eq!(
      parse_key_value("Max_Line_Length = 80"),
      Some(("max_line_length".into(), "80".into()))
    );
    assert_eq!(parse_key_value("no_equals_sign"), None);
  }
}
