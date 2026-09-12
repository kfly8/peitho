use std::{
    fs,
    ops::Range,
    path::{Component, Path, PathBuf},
};

use pulldown_cmark::{Event, Parser, Tag};
use serde_json::Value;

use crate::{
    domain::SourceSpan,
    error::{BuildError, ErrorKind},
    Result,
};

const MAX_INCLUDE_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpandedSource {
    pub source: String,
    pub body_start: usize,
    pub line_map: LineMap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCol {
    /// The 1-based line number.
    pub line: usize,
    /// The 0-based byte offset from the start of the line.
    pub byte_col: usize,
}

/// A half-open span whose bytes come from one origin file.
///
/// `combined` is the clipped span in the include-expanded source containing those same bytes.
/// `end` is exclusive and is expressed on the line of the last byte covered. If the span ends with
/// a line terminator, its `byte_col` equals that line's byte length including the terminator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginSpan {
    pub file: PathBuf,
    pub combined: SourceSpan,
    pub start: LineCol,
    pub end: LineCol,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMap {
    origins: Vec<LineOrigin>,
}

impl LineMap {
    pub fn len(&self) -> usize {
        self.origins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.origins.is_empty()
    }

    pub fn translate(&self, line: usize) -> (PathBuf, usize) {
        if line == 0 {
            return (PathBuf::new(), line);
        }

        let mut output_line = 0usize;
        for (index, origin) in self.origins.iter().enumerate() {
            match origin.kind {
                LineOriginKind::Source { line: source_line } => {
                    output_line += 1;
                    if output_line == line {
                        return (origin.file.clone(), source_line);
                    }
                }
                LineOriginKind::SyntheticLine => {
                    output_line += 1;
                    if output_line == line {
                        return self.translate_synthetic_origin(index);
                    }
                }
                LineOriginKind::SyntheticTerminator => {}
            }
        }

        (PathBuf::new(), line)
    }

    /// Translates a half-open combined-source byte span into one origin file.
    ///
    /// Synthetic units at either edge are clipped. Every remaining byte must map to `Source`
    /// entries of one file with consecutive line numbers. A span containing only synthetic bytes,
    /// an internal synthetic byte, mixed files, invalid bounds, or a map that does not match the
    /// source yields `None`.
    pub fn translate_span(&self, combined_source: &str, span: SourceSpan) -> Option<OriginSpan> {
        if span.start >= span.end
            || span.end > combined_source.len()
            || !combined_source.is_char_boundary(span.start)
            || !combined_source.is_char_boundary(span.end)
        {
            return None;
        }

        struct IntersectingUnit<'a> {
            origin: &'a LineOrigin,
            start: usize,
            end: usize,
        }

        let bytes = combined_source.as_bytes();
        let mut combined_offset = 0usize;
        let mut intersecting = Vec::new();

        for (index, origin) in self.origins.iter().enumerate() {
            if combined_offset >= bytes.len() {
                return None;
            }

            let unit_start = combined_offset;
            let unit_end = match origin.kind {
                LineOriginKind::Source { .. } => {
                    let newline = bytes[unit_start..].iter().position(|byte| *byte == b'\n');
                    // The newline after an unterminated source line belongs to the following
                    // SyntheticTerminator entry, rather than to this Source entry.
                    let has_synthetic_terminator = self
                        .origins
                        .get(index + 1)
                        .is_some_and(|next| next.kind == LineOriginKind::SyntheticTerminator);
                    match (newline, has_synthetic_terminator) {
                        (Some(newline), true) => unit_start + newline,
                        (Some(newline), false) => unit_start + newline + 1,
                        (None, true) => return None,
                        (None, false) => bytes.len(),
                    }
                }
                LineOriginKind::SyntheticTerminator => {
                    if bytes[unit_start] != b'\n' {
                        return None;
                    }
                    unit_start + 1
                }
                LineOriginKind::SyntheticLine => {
                    if bytes[unit_start] != b'\n' {
                        return None;
                    }
                    unit_start + 1
                }
            };
            combined_offset = unit_end;

            if span.start < unit_end && span.end > unit_start {
                intersecting.push(IntersectingUnit {
                    origin,
                    start: unit_start,
                    end: unit_end,
                });
            }
        }

        if combined_offset != bytes.len() {
            return None;
        }

        let first_source = intersecting
            .iter()
            .position(|unit| matches!(unit.origin.kind, LineOriginKind::Source { .. }))?;
        let last_source = intersecting
            .iter()
            .rposition(|unit| matches!(unit.origin.kind, LineOriginKind::Source { .. }))?;
        let mapped = &intersecting[first_source..=last_source];
        let first = mapped.first()?;
        let last = mapped.last()?;
        let LineOriginKind::Source { line: start_line } = first.origin.kind else {
            return None;
        };
        let LineOriginKind::Source { line: end_line } = last.origin.kind else {
            return None;
        };
        let file = first.origin.file.as_path();
        let mut previous_line: Option<usize> = None;

        for unit in mapped {
            let LineOriginKind::Source { line } = unit.origin.kind else {
                return None;
            };
            if unit.origin.file.as_path() != file {
                return None;
            }
            if previous_line.is_some_and(|previous_line| previous_line.checked_add(1) != Some(line))
            {
                return None;
            }
            previous_line = Some(line);
        }

        let combined = SourceSpan {
            start: span.start.max(first.start),
            end: span.end.min(last.end),
        };
        Some(OriginSpan {
            file: file.to_path_buf(),
            combined,
            start: LineCol {
                line: start_line,
                byte_col: combined.start - first.start,
            },
            end: LineCol {
                line: end_line,
                byte_col: combined.end - last.start,
            },
        })
    }

    pub fn origins(&self) -> &[LineOrigin] {
        &self.origins
    }

    fn translate_synthetic_origin(&self, synthetic_index: usize) -> (PathBuf, usize) {
        let synthetic = &self.origins[synthetic_index];
        self.origins
            .get(..synthetic_index)
            .unwrap_or_default()
            .iter()
            .rev()
            .find_map(|origin| match origin.kind {
                LineOriginKind::Source { line } => Some((origin.file.clone(), line)),
                LineOriginKind::SyntheticTerminator | LineOriginKind::SyntheticLine => None,
            })
            .unwrap_or_else(|| (synthetic.file.clone(), 1))
    }

    fn for_source(source: &str, path: &Path) -> Self {
        Self {
            origins: (1..=line_count(source))
                .map(|line| LineOrigin {
                    file: path.to_path_buf(),
                    kind: LineOriginKind::Source { line },
                })
                .collect(),
        }
    }
}

/// Indexes text as read from disk by 1-based line and 0-based byte column.
///
/// A leading BOM is skipped and the returned range is offset past it. Returns `None` for a
/// missing line or byte column, or for an empty or reversed range.
pub fn origin_span_to_range(origin_source: &str, span: &OriginSpan) -> Option<Range<usize>> {
    let source = strip_bom(origin_source);
    let bom_len = origin_source.len() - source.len();
    let start = bom_len + line_col_to_offset(source, span.start)?;
    let end = bom_len + line_col_to_offset(source, span.end)?;
    (start < end).then_some(start..end)
}

fn strip_bom(source: &str) -> &str {
    source.strip_prefix('\u{feff}').unwrap_or(source)
}

fn line_col_to_offset(source: &str, line_col: LineCol) -> Option<usize> {
    let source_line = source_lines_from(source, 0)
        .into_iter()
        .find(|source_line| source_line.line_no == line_col.line)?;
    if line_col.byte_col > source_line.end - source_line.start {
        return None;
    }

    let offset = source_line.start + line_col.byte_col;
    source.is_char_boundary(offset).then_some(offset)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineOrigin {
    pub file: PathBuf,
    pub kind: LineOriginKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineOriginKind {
    Source { line: usize },
    SyntheticTerminator,
    SyntheticLine,
}

/// Expands include directives, stripping one leading BOM from every source it reads.
///
/// `top_body_start` is interpreted against the BOM-stripped text, matching the parser.
pub fn expand_includes(
    top_source: &str,
    top_body_start: usize,
    top_path: &Path,
) -> Result<ExpandedSource> {
    let top_source = strip_bom(top_source);
    let mut stack = vec![path_key(top_path)];
    let deck_root = include_deck_root(top_path);
    expand_includes_for_source(top_source, top_body_start, top_path, &deck_root, &mut stack)
}

fn expand_includes_for_source(
    source_input: &str,
    body_start: usize,
    current_path: &Path,
    deck_root: &Path,
    stack: &mut Vec<PathBuf>,
) -> Result<ExpandedSource> {
    let regions = scan_slide_regions(source_input, body_start)
        .map_err(|err| err.with_origin_file(current_path))?;
    if regions.iter().all(|region| region.includes.is_empty()) {
        return Ok(ExpandedSource {
            source: source_input.to_owned(),
            body_start,
            line_map: LineMap::for_source(source_input, current_path),
        });
    }

    let mut source = String::new();
    let mut origins = Vec::new();
    let mut cursor = 0usize;
    for region in regions.iter().filter(|region| !region.includes.is_empty()) {
        append_chunk(
            &mut source,
            &mut origins,
            source_input,
            cursor,
            region.start,
            current_path,
        );
        // validate_include_region guarantees region.includes.len() == 1 before we reach here.
        let include = &region.includes[0];
        validate_include_target(current_path, &include.target, deck_root, include.line)
            .map_err(|err| err.with_origin_file(current_path))?;
        let include_path = resolve_include_path(current_path, &include.target);
        let include_key = path_key(&include_path);
        if let Some(position) = stack.iter().position(|path| path == &include_key) {
            return Err(
                include_cycle_error(include.line, stack, position, &include_key)
                    .with_origin_file(current_path),
            );
        }
        if stack.len() >= MAX_INCLUDE_DEPTH {
            return Err(include_depth_error(include.line).with_origin_file(current_path));
        }
        let included_source = fs::read_to_string(&include_path).map_err(|err| {
            include_read_error(&include.target, &include_path, include.line, err)
                .with_origin_file(current_path)
        })?;
        let included_source = strip_bom(&included_source);
        if source_has_no_content(included_source) {
            return Err(
                included_file_has_no_slides_error(&include.target, include.line)
                    .with_origin_file(current_path),
            );
        }
        if let Some(line) = crate::parser::detect_frontmatter_present(included_source) {
            return Err(included_frontmatter_error(line).with_origin_file(&include_path));
        }
        validate_included_source_boundary(included_source)
            .map_err(|err| err.with_origin_file(&include_path))?;
        stack.push(include_key);
        let expanded =
            expand_includes_for_source(included_source, 0, &include_path, deck_root, stack)?;
        stack.pop();
        append_region_leading_newline_if_needed(
            &mut source,
            &mut origins,
            source_input,
            region.start,
            &expanded,
            current_path,
        );
        source.push_str(&expanded.source);
        origins.extend(expanded.line_map.origins);
        append_separator_boundary_blank_line_if_needed(
            &mut source,
            &mut origins,
            source_input,
            region.end,
            current_path,
        );
        cursor = region.end;
    }
    append_chunk(
        &mut source,
        &mut origins,
        source_input,
        cursor,
        source_input.len(),
        current_path,
    );
    Ok(ExpandedSource {
        source,
        body_start,
        line_map: LineMap { origins },
    })
}

fn path_key(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| lexically_normalize(path))
}

fn lexically_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }
    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

fn include_deck_root(top_path: &Path) -> PathBuf {
    normalize_existing_path_for_include_check(parent_dir_or_dot(top_path))
}

fn parent_dir_or_dot(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn normalize_existing_path_for_include_check(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| lexically_normalize_preserving_parent(path))
}

fn lexically_normalize_preserving_parent(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match normalized.components().next_back() {
                Some(Component::Normal(_)) => {
                    normalized.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => normalized.push(".."),
            },
            Component::Normal(part) => normalized.push(part),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }
    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

fn append_region_leading_newline_if_needed(
    output: &mut String,
    origins: &mut Vec<LineOrigin>,
    source: &str,
    region_start: usize,
    replacement: &ExpandedSource,
    current_path: &Path,
) {
    if replacement.source.is_empty() || output.is_empty() || output.ends_with('\n') {
        return;
    }
    if source[region_start..].starts_with('\n') || source[region_start..].starts_with("\r\n") {
        append_synthetic_boundary_newline(output, origins, current_path);
    }
}

fn append_separator_boundary_blank_line_if_needed(
    output: &mut String,
    origins: &mut Vec<LineOrigin>,
    source: &str,
    next: usize,
    current_path: &Path,
) {
    let next_line = source[next..]
        .split_inclusive('\n')
        .next()
        .unwrap_or_default()
        .trim_end_matches('\n')
        .trim_end_matches('\r');
    if !is_slide_separator(next_line) {
        return;
    }
    while !ends_with_blank_line(output) {
        append_synthetic_boundary_newline(output, origins, current_path);
    }
}

fn append_synthetic_boundary_newline(
    output: &mut String,
    origins: &mut Vec<LineOrigin>,
    current_path: &Path,
) {
    let kind = if output.ends_with('\n') {
        LineOriginKind::SyntheticLine
    } else {
        LineOriginKind::SyntheticTerminator
    };
    output.push('\n');
    origins.push(synthetic_boundary_origin(current_path, kind));
}

fn ends_with_blank_line(source: &str) -> bool {
    source
        .strip_suffix('\n')
        .is_some_and(|source| source.trim_end_matches('\r').ends_with('\n'))
}

fn synthetic_boundary_origin(current_path: &Path, kind: LineOriginKind) -> LineOrigin {
    LineOrigin {
        file: current_path.to_path_buf(),
        kind,
    }
}

fn include_cycle_error(
    line: usize,
    stack: &[PathBuf],
    cycle_start: usize,
    target: &Path,
) -> BuildError {
    let mut parts = stack[cycle_start..]
        .iter()
        .map(|path| path_label(path))
        .collect::<Vec<_>>();
    parts.push(path_label(target));
    BuildError::new(
        ErrorKind::Parse,
        Some(line),
        format!("include cycle detected: {}", parts.join(" -> ")),
        "remove one of the include comments in the cycle",
    )
}

fn include_depth_error(line: usize) -> BuildError {
    BuildError::new(
        ErrorKind::Parse,
        Some(line),
        format!("include chain exceeds max depth of {MAX_INCLUDE_DEPTH}"),
        "reduce include nesting",
    )
}

fn path_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn line_count(source: &str) -> usize {
    source.lines().count()
}

#[derive(Debug)]
struct SlideRegion<'a> {
    start: usize,
    end: usize,
    lines: Vec<&'a str>,
    includes: Vec<IncludeDirective>,
}

#[derive(Debug)]
struct IncludeDirective {
    target: PathBuf,
    line: usize,
}

#[derive(Debug)]
struct SourceLine<'a> {
    line: &'a str,
    line_no: usize,
    start: usize,
    end: usize,
}

fn scan_slide_regions(source: &str, body_start: usize) -> Result<Vec<SlideRegion<'_>>> {
    let mut regions = Vec::new();
    let mut current = SlideRegion {
        start: body_start,
        end: body_start,
        lines: Vec::new(),
        includes: Vec::new(),
    };
    let mut in_code_fence: Option<(char, usize)> = None;

    for source_line in source_lines_from(source, body_start) {
        let trimmed = source_line.line.trim_end_matches('\r');

        if in_code_fence.is_none() && source_line.start >= body_start && is_slide_separator(trimmed)
        {
            current.end = source_line.start;
            validate_include_region(&current)?;
            regions.push(current);
            current = SlideRegion {
                start: source_line.end,
                end: source_line.end,
                lines: Vec::new(),
                includes: Vec::new(),
            };
            continue;
        }

        let include = if in_code_fence.is_none() {
            parse_include_comment(trimmed, source_line.line_no)?
        } else {
            None
        };

        current.lines.push(source_line.line);
        if let Some(include) = include {
            current.includes.push(include);
        }

        if let Some((fence_char, fence_len)) = in_code_fence {
            if crate::parser::is_closing_code_fence(trimmed, fence_char, fence_len) {
                in_code_fence = None;
            }
        } else if let Some((fence_char, fence_len)) = crate::parser::opening_code_fence(trimmed) {
            in_code_fence = Some((fence_char, fence_len));
        }
    }

    current.end = source.len();
    validate_include_region(&current)?;
    regions.push(current);
    Ok(regions)
}

fn source_lines_from(source: &str, body_start: usize) -> Vec<SourceLine<'_>> {
    let mut lines = Vec::new();
    let mut offset = 0usize;
    for (index, raw_line) in source.split_inclusive('\n').enumerate() {
        let start = offset;
        let end = start + raw_line.len();
        offset = end;
        if end <= body_start {
            continue;
        }
        let slice_start = body_start.saturating_sub(start);
        let line = raw_line[slice_start..]
            .strip_suffix('\n')
            .unwrap_or(&raw_line[slice_start..]);
        lines.push(SourceLine {
            line,
            line_no: index + 1,
            start,
            end,
        });
    }
    lines
}

fn validate_include_region(region: &SlideRegion<'_>) -> Result<()> {
    if region.includes.is_empty() {
        return Ok(());
    }
    let include = &region.includes[0];
    let significant_lines = region
        .lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .count();
    if region.includes.len() != 1 || significant_lines != 1 {
        return Err(include_container_error(include.line));
    }
    Ok(())
}

fn parse_include_comment(line: &str, line_no: usize) -> Result<Option<IncludeDirective>> {
    let trimmed = line.trim();
    let Some(json) = trimmed
        .strip_prefix("<!--")
        .and_then(|comment| comment.strip_suffix("-->"))
        .map(str::trim)
    else {
        return Ok(None);
    };
    if !json.contains("\"include\"") {
        return Ok(None);
    }

    let parsed: Value =
        serde_json::from_str(json).map_err(|err| invalid_include_comment_error(line_no, err))?;
    let Value::Object(object) = parsed else {
        return Err(BuildError::new(
            ErrorKind::Parse,
            Some(line_no),
            "include comment must be a JSON object",
            r#"use <!-- {"include":"path/to/slides.md"} -->"#,
        ));
    };
    if !object.contains_key("include") {
        return Ok(None);
    }
    if object.len() != 1 {
        return Err(BuildError::new(
            ErrorKind::Parse,
            Some(line_no),
            "include comment accepts only include",
            r#"use <!-- {"include":"path/to/slides.md"} --> on a slide with no other settings"#,
        ));
    }
    let Some(include) = object.get("include").and_then(Value::as_str) else {
        return Err(include_value_error(line_no));
    };
    if include.trim().is_empty() {
        return Err(include_value_error(line_no));
    }
    Ok(Some(IncludeDirective {
        target: PathBuf::from(include),
        line: line_no,
    }))
}

fn include_value_error(line: usize) -> BuildError {
    BuildError::new(
        ErrorKind::Parse,
        Some(line),
        "include value must be a non-empty string",
        r#"set "include" to a deck-relative Markdown file path"#,
    )
}

fn invalid_include_comment_error(line: usize, err: serde_json::Error) -> BuildError {
    BuildError::new(
        ErrorKind::Parse,
        Some(line),
        format!("invalid include comment: {err}"),
        r#"use <!-- {"include":"path/to/slides.md"} -->"#,
    )
}

fn include_container_error(line: usize) -> BuildError {
    BuildError::new(
        ErrorKind::Parse,
        Some(line),
        "include comment must be the only content of its slide",
        "place the include comment in its own slide bounded by `---`",
    )
}

fn resolve_include_path(current_path: &Path, target: &Path) -> PathBuf {
    parent_dir_or_dot(current_path).join(target)
}

fn validate_include_target(
    current_path: &Path,
    target: &Path,
    deck_root: &Path,
    line: usize,
) -> Result<()> {
    if target.is_absolute() {
        return Err(absolute_include_path_error(line));
    }

    let current_dir = normalize_existing_path_for_include_check(parent_dir_or_dot(current_path));
    let resolved = lexically_normalize_preserving_parent(&current_dir.join(target));
    if !resolved.starts_with(deck_root) {
        return Err(escaping_include_path_error(line));
    }
    Ok(())
}

fn absolute_include_path_error(line: usize) -> BuildError {
    BuildError::new(
        ErrorKind::Parse,
        Some(line),
        "include path must be deck-relative, not absolute",
        "use a path relative to the including file (e.g. `shared/intro.md`)",
    )
}

fn escaping_include_path_error(line: usize) -> BuildError {
    BuildError::new(
        ErrorKind::Parse,
        Some(line),
        "include path must not escape the deck directory",
        "keep includes within the deck's directory or a subdirectory",
    )
}

fn include_read_error(
    target: &Path,
    resolved: &Path,
    line: usize,
    err: std::io::Error,
) -> BuildError {
    match err.kind() {
        std::io::ErrorKind::NotFound => BuildError::new(
            ErrorKind::Parse,
            Some(line),
            format!("include file not found: {}", target.display()),
            "create the included file or fix the include path",
        ),
        std::io::ErrorKind::IsADirectory => include_directory_error(target, line),
        _ if resolved.is_dir() => include_directory_error(target, line),
        _ => BuildError::new(
            ErrorKind::Parse,
            Some(line),
            format!("failed to read include file {}: {err}", target.display()),
            "make the included file readable or fix the include path",
        ),
    }
}

fn include_directory_error(target: &Path, line: usize) -> BuildError {
    BuildError::new(
        ErrorKind::Parse,
        Some(line),
        format!(
            "include target is a directory, not a file: {}",
            target.display()
        ),
        "point include at a Markdown file, not a directory",
    )
}

fn included_file_has_no_slides_error(target: &Path, line: usize) -> BuildError {
    BuildError::new(
        ErrorKind::Parse,
        Some(line),
        format!("included file has no slides: {}", target.display()),
        "add at least one slide to the included file or remove the include comment",
    )
}

fn included_frontmatter_error(line: usize) -> BuildError {
    BuildError::new(
        ErrorKind::Parse,
        Some(line),
        "included file has frontmatter, which is not supported",
        "move frontmatter to the top-level deck; included files may only contain slides",
    )
}

fn validate_included_source_boundary(source: &str) -> Result<()> {
    const PROBE_PARAGRAPH: &str = "peitho include boundary probe";

    let source_end = source.len();
    let mut probe = String::with_capacity(source_end + PROBE_PARAGRAPH.len() + 7);
    probe.push_str(source);
    probe.push_str("\n\n---\n");
    probe.push_str(PROBE_PARAGRAPH);

    let mut spanning_event = None;
    for (event, range) in
        Parser::new_ext(&probe, crate::parser::slide_split_options()).into_offset_iter()
    {
        if matches!(&event, Event::Rule) && range.start >= source_end {
            return Ok(());
        }
        if spanning_event.is_none() && range.start < source_end && range.end > source_end {
            spanning_event = Some((event, range.start));
        }
    }

    let Some((event, opening_offset)) = spanning_event else {
        let line = crate::parser::line_for_offset(source, source_end.saturating_sub(1));
        return Err(included_unclosed_construct_error(
            line,
            "parser event boundary",
        ));
    };
    let line = crate::parser::line_for_offset(source, opening_offset);
    let construct = match event {
        Event::Start(Tag::CodeBlock(_)) => "code fence".to_owned(),
        Event::Start(Tag::HtmlBlock) | Event::Html(_) => "HTML block".to_owned(),
        Event::Start(tag) => format!("Markdown construct ({tag:?})"),
        Event::End(tag) => format!("Markdown construct (End({tag:?}))"),
        Event::Text(_) => "Markdown construct (Text)".to_owned(),
        Event::Code(_) => "Markdown construct (Code)".to_owned(),
        Event::InlineMath(_) => "Markdown construct (InlineMath)".to_owned(),
        Event::DisplayMath(_) => "Markdown construct (DisplayMath)".to_owned(),
        Event::InlineHtml(_) => "Markdown construct (InlineHtml)".to_owned(),
        Event::FootnoteReference(_) => "Markdown construct (FootnoteReference)".to_owned(),
        Event::SoftBreak => "Markdown construct (SoftBreak)".to_owned(),
        Event::HardBreak => "Markdown construct (HardBreak)".to_owned(),
        Event::Rule => "Markdown construct (Rule)".to_owned(),
        Event::TaskListMarker(_) => "Markdown construct (TaskListMarker)".to_owned(),
    };
    Err(included_unclosed_construct_error(line, &construct))
}

fn included_unclosed_construct_error(line: usize, construct: &str) -> BuildError {
    BuildError::new(
        ErrorKind::Parse,
        Some(line),
        format!("included file ends inside an unclosed {construct}"),
        "close it before the end of the included file",
    )
}

fn append_chunk(
    output: &mut String,
    origins: &mut Vec<LineOrigin>,
    source: &str,
    start: usize,
    end: usize,
    path: &Path,
) {
    if start == end {
        return;
    }
    let chunk = &source[start..end];
    let first_line = crate::parser::line_for_offset(source, start);
    output.push_str(chunk);
    origins.extend((0..line_count(chunk)).map(|index| LineOrigin {
        file: path.to_path_buf(),
        kind: LineOriginKind::Source {
            line: first_line + index,
        },
    }));
}

fn is_slide_separator(line: &str) -> bool {
    line.trim() == "---"
}

fn source_has_no_content(source: &str) -> bool {
    source.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use crate::domain::{FragmentKind, SlotName};

    use super::*;

    fn origin_span(
        file: &Path,
        combined: (usize, usize),
        start: (usize, usize),
        end: (usize, usize),
    ) -> OriginSpan {
        OriginSpan {
            file: file.to_path_buf(),
            combined: SourceSpan {
                start: combined.0,
                end: combined.1,
            },
            start: LineCol {
                line: start.0,
                byte_col: start.1,
            },
            end: LineCol {
                line: end.0,
                byte_col: end.1,
            },
        }
    }

    fn expand_fixture(
        dir: &Path,
        deck: &str,
        include_name: &str,
        include: &str,
    ) -> (PathBuf, PathBuf, ExpandedSource) {
        let deck_path = dir.join("deck.md");
        let include_path = dir.join(include_name);
        fs::write(&include_path, include).unwrap();
        let expanded = expand_includes(deck, 0, &deck_path).unwrap();
        (deck_path, include_path, expanded)
    }

    fn assert_parsed_slide_spans_translate(
        expanded: &ExpandedSource,
        expected_origins: &[(&Path, &str)],
    ) {
        let frontmatter = crate::parser::parse_frontmatter(&expanded.source).unwrap();
        let parsed = crate::parser::parse_markdown(
            &expanded.source,
            frontmatter,
            &crate::highlight::Highlighter::defaults(),
        )
        .unwrap();

        assert_eq!(parsed.parsed_slides().len(), expected_origins.len());
        for (slide, (expected_file, origin_source)) in
            parsed.parsed_slides().iter().zip(expected_origins)
        {
            let translated = expanded
                .line_map
                .translate_span(&expanded.source, slide.source_span)
                .expect("every parsed slide span should translate");
            assert_eq!(translated.file.as_path(), *expected_file);
            assert!(translated.combined.start >= slide.source_span.start);
            assert!(translated.combined.end <= slide.source_span.end);
            let origin_range = origin_span_to_range(origin_source, &translated)
                .expect("translated coordinates should index the origin");
            assert_eq!(
                &origin_source[origin_range],
                &expanded.source[translated.combined.start..translated.combined.end],
            );
        }
    }

    #[test]
    fn expand_includes_returns_source_unchanged_when_no_include_comment_is_present() {
        let source = "---\ntime: 1m\n---\n# Intro\n\nBody\n";
        let expanded = expand_includes(source, 17, Path::new("deck.md")).unwrap();

        assert_eq!(expanded.source, source);
        assert_eq!(expanded.body_start, 17);
        assert_eq!(expanded.line_map.len(), source.lines().count());
        assert_eq!(
            expanded.line_map.translate(4),
            (Path::new("deck.md").to_path_buf(), 4)
        );
    }

    #[test]
    fn expand_includes_strips_top_deck_bom_before_using_body_start() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        let included = dir.path().join("inc.md");
        let included_source = "# Inc\n";
        fs::write(&included, included_source).unwrap();
        let source = "\u{feff}---\ntime: 1m\n---\n<!-- {\"include\":\"inc.md\"} -->\n---\n# Top\n";
        let frontmatter = crate::parser::parse_frontmatter(source).unwrap();

        let expanded = expand_includes(source, frontmatter.body_start(), &deck).unwrap();

        assert!(expanded.source.starts_with("---\ntime: 1m\n---\n# Inc\n"));
        assert!(expanded.line_map.origins().iter().any(|origin| {
            origin.file == deck && origin.kind == LineOriginKind::SyntheticTerminator
        }));
        assert_parsed_slide_spans_translate(
            &expanded,
            &[
                (included.as_path(), included_source),
                (deck.as_path(), source),
            ],
        );
    }

    #[test]
    fn included_file_bom_is_stripped_before_splicing() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        let included = dir.path().join("inc.md");
        let included_source = "\u{feff}# Included\n\nbody\n";
        fs::write(&included, included_source).unwrap();
        let source = "# Top\n\n---\n<!-- {\"include\":\"inc.md\"} -->\n";

        let expanded = expand_includes(source, 0, &deck).unwrap();

        assert!(expanded.source.contains("---\n# Included\n\nbody\n"));
        assert!(!expanded.source.contains('\u{feff}'));
        assert_parsed_slide_spans_translate(
            &expanded,
            &[
                (deck.as_path(), source),
                (included.as_path(), included_source),
            ],
        );
    }

    #[test]
    fn translate_span_maps_plain_and_included_coordinates() {
        let dir = tempfile::tempdir().unwrap();
        let included_source = "# Included\n\n<!-- shared note -->\n";
        let source = "# Before\n\n---\n<!-- {\"include\":\"shared.md\"} -->\n\n---\n# After\n";
        let (_, included, expanded) =
            expand_fixture(dir.path(), source, "shared.md", included_source);

        let combined_start = expanded.source.find("<!-- shared note -->").unwrap();
        let origin_start = included_source.find("<!-- shared note -->").unwrap();
        let translated = expanded
            .line_map
            .translate_span(
                &expanded.source,
                SourceSpan {
                    start: combined_start,
                    end: combined_start + "<!-- shared note -->".len(),
                },
            )
            .unwrap();
        assert_eq!(translated.file, included);
        assert_eq!(
            translated.combined,
            SourceSpan {
                start: combined_start,
                end: combined_start + "<!-- shared note -->".len(),
            }
        );
        assert_eq!(
            translated.start,
            LineCol {
                line: 3,
                byte_col: 0
            }
        );
        assert_eq!(
            translated.end,
            LineCol {
                line: 3,
                byte_col: 20
            }
        );
        assert_eq!(
            origin_span_to_range(included_source, &translated),
            Some(origin_start..origin_start + "<!-- shared note -->".len()),
        );

        let plain_path = dir.path().join("plain.md");
        let plain_source = "# Plain\nsecond line\nlast";
        let plain = expand_includes(plain_source, 0, &plain_path).unwrap();
        let second_line_start = plain_source.find("second").unwrap();

        let multi_line = plain
            .line_map
            .translate_span(
                &plain.source,
                SourceSpan {
                    start: "# ".len(),
                    end: second_line_start + "second".len(),
                },
            )
            .unwrap();
        assert_eq!(
            multi_line,
            origin_span(
                &plain_path,
                ("# ".len(), second_line_start + "second".len()),
                (1, 2),
                (2, 6),
            )
        );

        let through_terminator = plain
            .line_map
            .translate_span(
                &plain.source,
                SourceSpan {
                    start: second_line_start,
                    end: second_line_start + "second line\n".len(),
                },
            )
            .unwrap();
        assert_eq!(
            through_terminator,
            origin_span(
                &plain_path,
                (second_line_start, second_line_start + "second line\n".len(),),
                (2, 0),
                (2, "second line\n".len()),
            )
        );

        let eof_start = plain_source.find("last").unwrap();
        let through_eof = plain
            .line_map
            .translate_span(
                &plain.source,
                SourceSpan {
                    start: eof_start,
                    end: plain.source.len(),
                },
            )
            .unwrap();
        assert_eq!(
            through_eof,
            origin_span(
                &plain_path,
                (eof_start, plain.source.len()),
                (3, 0),
                (3, "last".len()),
            )
        );

        let utf8_path = dir.path().join("utf8.md");
        let utf8_source = "# こんにちは\n";
        let utf8 = expand_includes(utf8_source, 0, &utf8_path).unwrap();
        let utf8_start = utf8_source.find('ん').unwrap();
        let utf8_end = utf8_start + "んに".len();
        let utf8_span = utf8
            .line_map
            .translate_span(
                &utf8.source,
                SourceSpan {
                    start: utf8_start,
                    end: utf8_end,
                },
            )
            .unwrap();
        assert_eq!(
            utf8_span,
            origin_span(&utf8_path, (utf8_start, utf8_end), (1, 5), (1, 11))
        );
        assert_eq!(
            origin_span_to_range(utf8_source, &utf8_span),
            Some(utf8_start..utf8_end)
        );
    }

    #[test]
    fn translate_span_maps_every_parsed_slide_span_of_include_decks() {
        let dir = tempfile::tempdir().unwrap();

        let trailing_included_source = "# Included\n\n<!-- note -->\n";
        let trailing_deck_source =
            "# Before\n\n---\n<!-- {\"include\":\"trailing-shared.md\"} -->\n\n---\n# After\n";
        let (trailing_deck, trailing_included, trailing_expanded) = expand_fixture(
            dir.path(),
            trailing_deck_source,
            "trailing-shared.md",
            trailing_included_source,
        );
        assert_parsed_slide_spans_translate(
            &trailing_expanded,
            &[
                (trailing_deck.as_path(), trailing_deck_source),
                (trailing_included.as_path(), trailing_included_source),
                (trailing_deck.as_path(), trailing_deck_source),
            ],
        );

        let leading_deck = dir.path().join("leading-deck.md");
        let leading_included = dir.path().join("leading-shared.md");
        let leading_included_source = "# Included\n";
        fs::write(&leading_included, leading_included_source).unwrap();
        let leading_deck_source =
            "---\ntime: 1m\n---\n<!-- {\"include\":\"leading-shared.md\"} -->\n---\n# Plain\n";
        let frontmatter = crate::parser::parse_frontmatter(leading_deck_source).unwrap();
        let leading_expanded =
            expand_includes(leading_deck_source, frontmatter.body_start(), &leading_deck).unwrap();
        assert_parsed_slide_spans_translate(
            &leading_expanded,
            &[
                (leading_included.as_path(), leading_included_source),
                (leading_deck.as_path(), leading_deck_source),
            ],
        );

        let plain_deck = dir.path().join("plain-deck.md");
        let plain_source = "# Plain\n\n<!-- note -->\n";
        let plain_expanded = expand_includes(plain_source, 0, &plain_deck).unwrap();
        assert_parsed_slide_spans_translate(
            &plain_expanded,
            &[(plain_deck.as_path(), plain_source)],
        );
    }

    #[test]
    fn origin_span_to_range_maps_utf8_and_crlf() {
        let crlf_source = "# T\r\n\r\n<!-- n -->\r\n";
        let crlf_start = crlf_source.find("<!-- n -->").unwrap();
        let crlf_span = origin_span(Path::new("crlf.md"), (0, 1), (3, 0), (3, 12));
        assert_eq!(
            origin_span_to_range(crlf_source, &crlf_span),
            Some(crlf_start..crlf_start + "<!-- n -->\r\n".len())
        );

        let bom_source = "\u{feff}# T\n\n<!-- n -->\n";
        let bom_first_line_span = OriginSpan {
            file: Path::new("bom.md").to_path_buf(),
            combined: SourceSpan {
                start: 0,
                end: "# T\n".len(),
            },
            start: LineCol {
                line: 1,
                byte_col: 0,
            },
            end: LineCol {
                line: 1,
                byte_col: "# T\n".len(),
            },
        };
        assert_eq!(
            origin_span_to_range(bom_source, &bom_first_line_span),
            Some('\u{feff}'.len_utf8()..'\u{feff}'.len_utf8() + "# T\n".len())
        );

        let bom_note_start = bom_source.find("<!-- n -->").unwrap();
        let bom_note_span = OriginSpan {
            file: Path::new("bom.md").to_path_buf(),
            combined: SourceSpan {
                start: bom_note_start - '\u{feff}'.len_utf8(),
                end: bom_note_start - '\u{feff}'.len_utf8() + "<!-- n -->\n".len(),
            },
            start: LineCol {
                line: 3,
                byte_col: 0,
            },
            end: LineCol {
                line: 3,
                byte_col: "<!-- n -->\n".len(),
            },
        };
        let bom_note_range = origin_span_to_range(bom_source, &bom_note_span).unwrap();
        assert_eq!(
            bom_note_range,
            bom_note_start..bom_note_start + "<!-- n -->\n".len()
        );
        assert_eq!(&bom_source[bom_note_range], "<!-- n -->\n");

        let utf8_source = "# こんにちは\n";
        let utf8_start = utf8_source.find('ん').unwrap();
        let utf8_span = origin_span(Path::new("utf8.md"), (0, 1), (1, 5), (1, 11));
        assert_eq!(
            origin_span_to_range(utf8_source, &utf8_span),
            Some(utf8_start..utf8_start + "んに".len())
        );

        let inside_multibyte = origin_span(Path::new("utf8.md"), (0, 1), (1, 3), (1, 5));
        assert_eq!(origin_span_to_range(utf8_source, &inside_multibyte), None);

        let missing_line = origin_span(Path::new("crlf.md"), (0, 1), (4, 0), (4, 1));
        assert_eq!(origin_span_to_range(crlf_source, &missing_line), None);

        let column_past_line = origin_span(Path::new("crlf.md"), (0, 1), (3, 0), (3, 13));
        assert_eq!(origin_span_to_range(crlf_source, &column_past_line), None);

        let empty = origin_span(Path::new("crlf.md"), (0, 1), (3, 5), (3, 5));
        assert_eq!(origin_span_to_range(crlf_source, &empty), None);

        let reversed = origin_span(Path::new("crlf.md"), (0, 1), (3, 6), (3, 5));
        assert_eq!(origin_span_to_range(crlf_source, &reversed), None);
    }

    #[test]
    fn translate_span_clips_trailing_synthetic_units() {
        let dir = tempfile::tempdir().unwrap();
        let source = "# Before\n\n---\n<!-- {\"include\":\"shared.md\"} -->\n---\n# After\n";
        let (_, included, expanded) = expand_fixture(dir.path(), source, "shared.md", "# Included");
        let included_start = expanded.source.find("# Included").unwrap();
        let generated_newline = included_start + "# Included".len();
        let following_separator = expanded.source[generated_newline + 2..]
            .find("---")
            .unwrap()
            + generated_newline
            + 2;

        assert_eq!(
            expanded
                .line_map
                .translate_span(
                    &expanded.source,
                    SourceSpan {
                        start: included_start,
                        end: following_separator,
                    },
                )
                .unwrap(),
            origin_span(
                &included,
                (included_start, generated_newline),
                (1, 0),
                (1, "# Included".len()),
            )
        );
    }

    #[test]
    fn translate_span_refuses_generated_boundary_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let source = "# Before\n\n---\n<!-- {\"include\":\"shared.md\"} -->\n---\n# After\n";
        let (deck, included, expanded) =
            expand_fixture(dir.path(), source, "shared.md", "# Included");

        let included_origin = expanded
            .line_map
            .origins()
            .iter()
            .position(|origin| {
                origin.file == included && origin.kind == LineOriginKind::Source { line: 1 }
            })
            .unwrap();
        assert_eq!(
            &expanded.line_map.origins()[included_origin - 1..included_origin + 3],
            &[
                LineOrigin {
                    file: deck.clone(),
                    kind: LineOriginKind::Source { line: 3 },
                },
                LineOrigin {
                    file: included.clone(),
                    kind: LineOriginKind::Source { line: 1 },
                },
                LineOrigin {
                    file: deck.clone(),
                    kind: LineOriginKind::SyntheticTerminator,
                },
                LineOrigin {
                    file: deck.clone(),
                    kind: LineOriginKind::SyntheticLine,
                },
            ]
        );

        let generated_newline = expanded.source.find("# Included").unwrap() + "# Included".len();
        // This byte is the SyntheticTerminator after an unterminated included line.
        assert_eq!(
            expanded.line_map.translate_span(
                &expanded.source,
                SourceSpan {
                    start: generated_newline,
                    end: generated_newline + 1
                },
            ),
            None
        );

        let included_start = expanded.source.find("# Included").unwrap();
        // This span contains only the SyntheticLine blank-line byte.
        assert_eq!(
            expanded.line_map.translate_span(
                &expanded.source,
                SourceSpan {
                    start: generated_newline + 1,
                    end: generated_newline + 2,
                },
            ),
            None
        );

        let after_start = expanded.source.find("# After").unwrap();
        // A synthetic byte strictly inside a span is reachable here only while also crossing from
        // the included file into the top deck, so this is both an internal-synthetic and mixed-file
        // refusal.
        assert_eq!(
            expanded.line_map.translate_span(
                &expanded.source,
                SourceSpan {
                    start: generated_newline - 1,
                    end: after_start + 1,
                },
            ),
            None
        );

        // These bytes cross from a top-deck Source entry into an included-file Source entry.
        assert_eq!(
            expanded.line_map.translate_span(
                &expanded.source,
                SourceSpan {
                    start: included_start - 1,
                    end: included_start + 1,
                },
            ),
            None
        );

        // This span is empty, so start is equal to end.
        assert_eq!(
            expanded.line_map.translate_span(
                &expanded.source,
                SourceSpan {
                    start: included_start,
                    end: included_start,
                },
            ),
            None
        );

        // This span is reversed, so start is greater than end.
        assert_eq!(
            expanded.line_map.translate_span(
                &expanded.source,
                SourceSpan {
                    start: included_start + 1,
                    end: included_start,
                },
            ),
            None
        );

        // This span extends beyond the end of the combined source.
        assert_eq!(
            expanded.line_map.translate_span(
                &expanded.source,
                SourceSpan {
                    start: expanded.source.len(),
                    end: expanded.source.len() + 1,
                },
            ),
            None
        );

        let utf8_source = "# こんにちは\n";
        let utf8 = expand_includes(utf8_source, 0, Path::new("utf8.md")).unwrap();
        let first_multibyte = utf8_source.find('こ').unwrap();
        // This span starts inside a UTF-8 code point rather than at a character boundary.
        assert_eq!(
            utf8.line_map.translate_span(
                &utf8.source,
                SourceSpan {
                    start: first_multibyte + 1,
                    end: utf8.source.len(),
                },
            ),
            None
        );
        // This span ends inside a UTF-8 code point rather than at a character boundary.
        assert_eq!(
            utf8.line_map.translate_span(
                &utf8.source,
                SourceSpan {
                    start: 0,
                    end: first_multibyte + 1,
                },
            ),
            None
        );

        let mut mismatched_map = utf8.line_map.clone();
        mismatched_map.origins.pop();
        // The origin walk ends before the combined source, so the map is inconsistent.
        assert_eq!(
            mismatched_map.translate_span(
                &utf8.source,
                SourceSpan {
                    start: 0,
                    end: "# ".len(),
                },
            ),
            None
        );

        let nonconsecutive_source = "# One\n# Two\n";
        let mut nonconsecutive_map =
            LineMap::for_source(nonconsecutive_source, Path::new("nonconsecutive.md"));
        nonconsecutive_map.origins[1].kind = LineOriginKind::Source { line: 3 };
        // These same-file Source entries have non-consecutive original line numbers.
        assert_eq!(
            nonconsecutive_map.translate_span(
                nonconsecutive_source,
                SourceSpan {
                    start: 0,
                    end: nonconsecutive_source.len(),
                },
            ),
            None
        );
    }

    #[test]
    fn include_comment_with_sibling_heading_is_an_error() {
        let source = "# Container\n\n<!-- {\"include\":\"shared.md\"} -->\n";
        let err = expand_includes(source, 0, Path::new("deck.md")).unwrap_err();

        assert_eq!(err.line, Some(3));
        assert!(err
            .message
            .contains("include comment must be the only content of its slide"));
        assert!(err
            .help
            .contains("place the include comment in its own slide"));
    }

    #[test]
    fn include_comment_with_sibling_page_settings_comment_is_an_error() {
        let source = "<!-- {\"key\":\"container\"} -->\n<!-- {\"include\":\"shared.md\"} -->\n";
        let err = expand_includes(source, 0, Path::new("deck.md")).unwrap_err();

        assert_eq!(err.line, Some(2));
        assert!(err
            .message
            .contains("include comment must be the only content of its slide"));
    }

    #[test]
    fn include_comment_with_sibling_speaker_note_is_an_error() {
        let source = "<!-- presenter note -->\n<!-- {\"include\":\"shared.md\"} -->\n";
        let err = expand_includes(source, 0, Path::new("deck.md")).unwrap_err();

        assert_eq!(err.line, Some(2));
        assert!(err
            .message
            .contains("include comment must be the only content of its slide"));
    }

    #[test]
    fn two_include_comments_in_the_same_slide_are_an_error() {
        let source = "<!-- {\"include\":\"a.md\"} -->\n<!-- {\"include\":\"b.md\"} -->\n";
        let err = expand_includes(source, 0, Path::new("deck.md")).unwrap_err();

        assert_eq!(err.line, Some(1));
        assert!(err
            .message
            .contains("include comment must be the only content of its slide"));
    }

    #[test]
    fn include_comment_rejects_page_settings_keys_on_the_same_comment() {
        for (name, extra) in [
            ("draft", r#""draft":true"#),
            ("skip", r#""skip":true"#),
            ("section", r#""section":"Intro","time":"1m""#),
            ("layout", r#""layout":"cover""#),
            ("key", r#""key":"intro""#),
            ("page_number", r#""page_number":false"#),
        ] {
            let source = format!(r#"<!-- {{"include":"shared.md",{extra}}} -->"#);
            let err = expand_includes(&source, 0, Path::new("deck.md")).unwrap_err();

            assert_eq!(err.line, Some(1), "{name}");
            assert!(
                err.message.contains("include comment accepts only include"),
                "{name}: {}",
                err.message
            );
        }
    }

    #[test]
    fn include_comment_rejects_non_string_include_value() {
        let source = r#"<!-- {"include":42} -->"#;
        let err = expand_includes(source, 0, Path::new("deck.md")).unwrap_err();

        assert_eq!(err.line, Some(1));
        assert!(err
            .message
            .contains("include value must be a non-empty string"));
    }

    #[test]
    fn include_comment_rejects_empty_include_value() {
        let source = r#"<!-- {"include":""} -->"#;
        let err = expand_includes(source, 0, Path::new("deck.md")).unwrap_err();

        assert_eq!(err.line, Some(1));
        assert!(err
            .message
            .contains("include value must be a non-empty string"));
    }

    #[test]
    fn include_comment_rejects_invalid_json() {
        let source = r#"<!-- {"include":} -->"#;
        let err = expand_includes(source, 0, Path::new("deck.md")).unwrap_err();

        assert_eq!(err.line, Some(1));
        assert!(err.message.contains("invalid include comment"));
    }

    #[test]
    fn include_comment_inside_fenced_code_block_is_preserved() {
        let source =
            "```markdown\n<!-- {\"include\":\"shared.md\",\"layout\":\"cover\"} -->\n```\n";
        let expanded = expand_includes(source, 0, Path::new("deck.md")).unwrap();

        assert_eq!(expanded.source, source);
    }

    #[test]
    fn malformed_non_include_page_comment_is_left_for_the_parser() {
        let source = r#"<!-- {"key":} -->"#;
        let expanded = expand_includes(source, 0, Path::new("deck.md")).unwrap();

        assert_eq!(expanded.source, source);
    }

    #[test]
    fn missing_include_target_is_a_line_numbered_error() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        let source = "# Before\n\n---\n<!-- {\"include\":\"missing.md\"} -->\n---\n# After\n";

        let err = expand_includes(source, 0, &deck).unwrap_err();

        assert_eq!(err.line, Some(4));
        assert!(err.message.contains("include file not found"));
        assert!(err.message.contains("missing.md"));
    }

    #[test]
    fn directory_include_target_is_a_friendly_line_numbered_error() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        fs::create_dir(dir.path().join("shared")).unwrap();
        let source = "<!-- {\"include\":\"shared\"} -->\n";

        let err = expand_includes(source, 0, &deck).unwrap_err();

        assert_eq!(err.line, Some(1));
        assert_eq!(err.origin_file, Some(deck));
        assert_eq!(
            err.message,
            "include target is a directory, not a file: shared"
        );
        assert_eq!(
            err.help,
            "point include at a Markdown file, not a directory"
        );
    }

    #[test]
    fn included_file_with_frontmatter_is_a_line_numbered_error() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        fs::write(
            dir.path().join("shared.md"),
            "---\ntime: 1m\n---\n# Included\n",
        )
        .unwrap();
        let source = "<!-- {\"include\":\"shared.md\"} -->\n";

        let err = expand_includes(source, 0, &deck).unwrap_err();

        assert_eq!(err.line, Some(1));
        assert_eq!(
            err.message,
            "included file has frontmatter, which is not supported"
        );
        assert!(err.help.contains("move frontmatter to the top-level deck"));
    }

    #[test]
    fn included_file_with_malformed_frontmatter_is_a_line_numbered_error() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        fs::write(dir.path().join("shared.md"), "---\ntime: 1m").unwrap();
        let source = "<!-- {\"include\":\"shared.md\"} -->\n";

        let err = expand_includes(source, 0, &deck).unwrap_err();

        assert_eq!(err.line, Some(1));
        assert_eq!(err.origin_file, Some(dir.path().join("shared.md")));
        assert_eq!(
            err.message,
            "included file has frontmatter, which is not supported"
        );
    }

    #[test]
    fn empty_included_file_is_a_line_numbered_error() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        fs::write(dir.path().join("shared.md"), " \n\t\n").unwrap();
        let source = "# Before\n\n---\n<!-- {\"include\":\"shared.md\"} -->\n---\n# After\n";

        let err = expand_includes(source, 0, &deck).unwrap_err();

        assert_eq!(err.line, Some(4));
        assert_eq!(err.origin_file, Some(deck));
        assert_eq!(err.message, "included file has no slides: shared.md");
        assert!(err.help.contains("add at least one slide"));
    }

    #[test]
    fn absolute_include_target_is_a_line_numbered_error() {
        let source = "# Before\n\n---\n<!-- {\"include\":\"/etc/passwd\"} -->\n";

        let err = expand_includes(source, 0, Path::new("deck.md")).unwrap_err();

        assert_eq!(err.line, Some(4));
        assert_eq!(err.origin_file, Some(Path::new("deck.md").to_path_buf()));
        assert_eq!(
            err.message,
            "include path must be deck-relative, not absolute"
        );
        assert_eq!(
            err.help,
            "use a path relative to the including file (e.g. `shared/intro.md`)"
        );
    }

    #[test]
    fn parent_directory_include_escape_is_a_line_numbered_error() {
        let dir = tempfile::tempdir().unwrap();
        let deck_dir = dir.path().join("dir");
        fs::create_dir(&deck_dir).unwrap();
        let deck = deck_dir.join("deck.md");
        fs::write(dir.path().join("secret.md"), "# Secret\n").unwrap();
        let source = "<!-- {\"include\":\"../secret.md\"} -->\n";

        let err = expand_includes(source, 0, &deck).unwrap_err();

        assert_eq!(err.line, Some(1));
        assert_eq!(err.origin_file, Some(deck));
        assert_eq!(
            err.message,
            "include path must not escape the deck directory"
        );
        assert_eq!(
            err.help,
            "keep includes within the deck's directory or a subdirectory"
        );
    }

    #[test]
    fn subdirectory_include_under_deck_root_is_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        let subdir = dir.path().join("subdir");
        fs::create_dir(&subdir).unwrap();
        fs::write(subdir.join("foo.md"), "# Included\n").unwrap();
        let source = "<!-- {\"include\":\"subdir/foo.md\"} -->\n";

        let expanded = expand_includes(source, 0, &deck).unwrap();

        assert_eq!(expanded.source, "# Included\n");
        assert_eq!(
            expanded.line_map.translate(1),
            (dir.path().join("subdir/foo.md"), 1)
        );
    }

    #[test]
    fn nested_parent_directory_include_escape_uses_top_deck_root() {
        let dir = tempfile::tempdir().unwrap();
        let deck_dir = dir.path().join("dir");
        fs::create_dir(&deck_dir).unwrap();
        let deck = deck_dir.join("deck.md");
        let included = deck_dir.join("a.md");
        fs::write(&included, "<!-- {\"include\":\"../B.md\"} -->\n").unwrap();
        fs::write(dir.path().join("B.md"), "# B\n").unwrap();
        let source = "<!-- {\"include\":\"a.md\"} -->\n";

        let err = expand_includes(source, 0, &deck).unwrap_err();

        assert_eq!(err.line, Some(1));
        assert_eq!(err.origin_file, Some(included));
        assert_eq!(
            err.message,
            "include path must not escape the deck directory"
        );
        assert_eq!(
            err.help,
            "keep includes within the deck's directory or a subdirectory"
        );
    }

    #[test]
    fn single_file_include_splices_slides_and_removes_container_slide() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        let included = dir.path().join("shared.md");
        fs::write(
            &included,
            "# Included One\n\n---\n<!-- {\"section\":\"Shared\",\"time\":\"1m\"} -->\n# Included Two\n",
        )
        .unwrap();
        let source = "# Before\n\n---\n<!-- {\"include\":\"shared.md\"} -->\n---\n# After\n";

        let expanded = expand_includes(source, 0, &deck).unwrap();

        assert_eq!(
            expanded.source,
            "# Before\n\n---\n# Included One\n\n---\n<!-- {\"section\":\"Shared\",\"time\":\"1m\"} -->\n# Included Two\n\n---\n# After\n"
        );
        assert_eq!(
            expanded.line_map.translate(4),
            (included.clone(), 1),
            "included slide should retain its source file line"
        );
    }

    #[test]
    fn include_ending_in_paragraph_keeps_following_slide() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        fs::write(
            dir.path().join("shared.md"),
            "# Included\n\nIncluded paragraph\n",
        )
        .unwrap();
        let source = "# Before\n\n---\n<!-- {\"include\":\"shared.md\"} -->\n---\n# After\n";

        let expanded = expand_includes(source, 0, &deck).unwrap();
        let frontmatter = crate::parser::parse_frontmatter(&expanded.source).unwrap();
        let parsed = crate::parser::parse_markdown(
            &expanded.source,
            frontmatter,
            &crate::highlight::Highlighter::defaults(),
        )
        .unwrap();

        assert_eq!(parsed.parsed_slides().len(), 3);
        assert_eq!(parsed.parsed_slides()[2].fragments[0].markdown(), "# After");
    }

    #[test]
    fn include_ending_in_list_keeps_following_slide() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        fs::write(
            dir.path().join("shared.md"),
            "# Included\n\n- Included item\n",
        )
        .unwrap();
        let source = "# Before\n\n---\n<!-- {\"include\":\"shared.md\"} -->\n---\n# After\n";

        let expanded = expand_includes(source, 0, &deck).unwrap();
        let frontmatter = crate::parser::parse_frontmatter(&expanded.source).unwrap();
        let parsed = crate::parser::parse_markdown(
            &expanded.source,
            frontmatter,
            &crate::highlight::Highlighter::defaults(),
        )
        .unwrap();

        assert_eq!(parsed.parsed_slides().len(), 3);
        assert_eq!(parsed.parsed_slides()[2].fragments[0].markdown(), "# After");
    }

    #[test]
    fn include_ending_in_closed_fence_keeps_following_slide() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        fs::write(
            dir.path().join("shared.md"),
            "# Included\n\n```\nincluded code\n```\n",
        )
        .unwrap();
        let source = "# Before\n\n---\n<!-- {\"include\":\"shared.md\"} -->\n---\n# After\n";

        let expanded = expand_includes(source, 0, &deck).unwrap();
        let frontmatter = crate::parser::parse_frontmatter(&expanded.source).unwrap();
        let parsed = crate::parser::parse_markdown(
            &expanded.source,
            frontmatter,
            &crate::highlight::Highlighter::defaults(),
        )
        .unwrap();

        assert_eq!(parsed.parsed_slides().len(), 3);
        assert_eq!(parsed.parsed_slides()[2].fragments[0].markdown(), "# After");
    }

    #[test]
    fn line_map_after_include_maps_following_slide_error_to_deck_line() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        fs::write(
            dir.path().join("shared.md"),
            "# Included\n\nIncluded paragraph\n",
        )
        .unwrap();
        let source = "<!-- {\"include\":\"shared.md\"} -->
---
# After

<div>unsupported</div>
";

        let expanded = expand_includes(source, 0, &deck).unwrap();
        let frontmatter = crate::parser::parse_frontmatter(&expanded.source).unwrap();
        let err = crate::parser::parse_markdown(
            &expanded.source,
            frontmatter,
            &crate::highlight::Highlighter::defaults(),
        )
        .unwrap_err();
        let (origin_file, original_line) = expanded.line_map.translate(err.line.unwrap());

        assert_eq!(origin_file, deck);
        assert_eq!(original_line, 5);
    }

    #[test]
    fn unclosed_fence_in_included_file_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        let included = dir.path().join("shared.md");
        fs::write(&included, "# Included\n\n```rust\nfn included() {}\n").unwrap();
        let source = "<!-- {\"include\":\"shared.md\"} -->\n";

        let err = expand_includes(source, 0, &deck).unwrap_err();

        assert_eq!(err.line, Some(3));
        assert_eq!(err.origin_file, Some(included));
        assert_eq!(
            err.message,
            "included file ends inside an unclosed code fence"
        );
        assert_eq!(err.help, "close it before the end of the included file");
    }

    #[test]
    fn included_bom_does_not_bypass_boundary_validation() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        let included = dir.path().join("inc.md");
        let source = "<!-- {\"include\":\"inc.md\"} -->\n---\n# After\n";

        fs::write(&included, "```\ncode").unwrap();
        let without_bom = expand_includes(source, 0, &deck).unwrap_err();
        fs::write(&included, "\u{feff}```\ncode").unwrap();
        let with_bom = expand_includes(source, 0, &deck).unwrap_err();

        assert_eq!(with_bom.line, Some(1));
        assert_eq!(with_bom.line, without_bom.line);
        assert_eq!(with_bom.origin_file, Some(included));
        assert_eq!(with_bom.message, without_bom.message);
        assert_eq!(
            with_bom.message,
            "included file ends inside an unclosed code fence"
        );
        assert_eq!(with_bom.help, without_bom.help);
    }

    #[test]
    fn unclosed_html_comment_in_included_file_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        let included = dir.path().join("shared.md");
        fs::write(&included, "# Included\n\n<!-- oops\n").unwrap();
        let source = "<!-- {\"include\":\"shared.md\"} -->\n";

        let err = expand_includes(source, 0, &deck).unwrap_err();

        assert_eq!(err.line, Some(3));
        assert_eq!(err.origin_file, Some(included));
        assert_eq!(
            err.message,
            "included file ends inside an unclosed HTML block"
        );
        assert_eq!(err.help, "close it before the end of the included file");
    }

    #[test]
    fn four_space_indented_backticks_do_not_open_a_fence() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        fs::write(
            dir.path().join("shared.md"),
            "# Included\n\n    ```\n\nIncluded paragraph\n",
        )
        .unwrap();
        let source = "# Before\n\n---\n<!-- {\"include\":\"shared.md\"} -->\n---\n# After\n";

        let expanded = expand_includes(source, 0, &deck).unwrap();
        let frontmatter = crate::parser::parse_frontmatter(&expanded.source).unwrap();
        let parsed = crate::parser::parse_markdown(
            &expanded.source,
            frontmatter,
            &crate::highlight::Highlighter::defaults(),
        )
        .unwrap();

        assert_eq!(parsed.parsed_slides().len(), 3);
        assert_eq!(parsed.parsed_slides()[2].fragments[0].markdown(), "# After");
    }

    #[test]
    fn backticks_inside_closed_html_comment_do_not_open_a_fence() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        fs::write(
            dir.path().join("shared.md"),
            "# Included\n\n<!--\n```\n-->\n\nIncluded paragraph\n",
        )
        .unwrap();
        let source = "# Before\n\n---\n<!-- {\"include\":\"shared.md\"} -->\n---\n# After\n";

        let expanded = expand_includes(source, 0, &deck).unwrap();
        let frontmatter = crate::parser::parse_frontmatter(&expanded.source).unwrap();
        let parsed = crate::parser::parse_markdown(
            &expanded.source,
            frontmatter,
            &crate::highlight::Highlighter::defaults(),
        )
        .unwrap();

        assert_eq!(parsed.parsed_slides().len(), 3);
        assert_eq!(parsed.parsed_slides()[2].fragments[0].markdown(), "# After");
    }

    #[test]
    fn unclosed_cdata_in_included_file_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        let included = dir.path().join("shared.md");
        fs::write(&included, "# Included\n\n<![CDATA[\noops\n").unwrap();
        let source = "<!-- {\"include\":\"shared.md\"} -->\n";

        let err = expand_includes(source, 0, &deck).unwrap_err();

        assert_eq!(err.line, Some(3));
        assert_eq!(err.origin_file, Some(included));
        assert_eq!(
            err.message,
            "included file ends inside an unclosed HTML block"
        );
        assert_eq!(err.help, "close it before the end of the included file");
    }

    #[test]
    fn include_without_trailing_newline_keeps_blank_line_before_following_separator() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        fs::write(dir.path().join("shared.md"), "# Included").unwrap();
        let source = "# Before\n\n---\n<!-- {\"include\":\"shared.md\"} -->\n---\n# After\n";

        let expanded = expand_includes(source, 0, &deck).unwrap();

        assert_eq!(
            expanded.source,
            "# Before\n\n---\n# Included\n\n---\n# After\n"
        );
        assert_eq!(expanded.line_map.translate(7), (deck, 6));
    }

    #[test]
    fn include_after_top_frontmatter_keeps_frontmatter_separator_on_its_own_line() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        fs::write(dir.path().join("shared.md"), "# Included\n").unwrap();
        let source = "---\ntime: 1m\n---\n<!-- {\"include\":\"shared.md\"} -->\n";
        let frontmatter = crate::parser::parse_frontmatter(source).unwrap();

        let expanded = expand_includes(source, frontmatter.body_start(), &deck).unwrap();

        assert_eq!(expanded.source, "---\ntime: 1m\n---\n# Included\n");
        assert_eq!(
            expanded.line_map.translate(4),
            (dir.path().join("shared.md"), 1)
        );
        assert!(expanded.line_map.origins.iter().any(|origin| {
            origin.file == deck && origin.kind == LineOriginKind::SyntheticTerminator
        }));
    }

    #[test]
    fn include_after_crlf_frontmatter_keeps_separator_on_its_own_line() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        let included = dir.path().join("shared.md");
        let included_source = "# Included\n";
        fs::write(&included, included_source).unwrap();
        let source =
            "---\r\ntime: 1m\r\n---\r\n<!-- {\"include\":\"shared.md\"} -->\r\n---\r\n# Plain\r\n";
        let frontmatter = crate::parser::parse_frontmatter(source).unwrap();

        let expanded = expand_includes(source, frontmatter.body_start(), &deck).unwrap();

        assert!(expanded
            .source
            .starts_with("---\r\ntime: 1m\r\n---\n# Included\n"));
        let closing_separator_end = "---\r\ntime: 1m\r\n---".len();
        assert_eq!(
            &expanded.source[closing_separator_end..closing_separator_end + 1],
            "\n"
        );
        assert!(expanded.line_map.origins().iter().any(|origin| {
            origin.file == deck && origin.kind == LineOriginKind::SyntheticTerminator
        }));
        assert_parsed_slide_spans_translate(
            &expanded,
            &[
                (included.as_path(), included_source),
                (deck.as_path(), source),
            ],
        );
    }

    #[test]
    fn expanded_source_with_included_section_marker_parses_as_one_deck() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        fs::write(
            dir.path().join("shared.md"),
            "<!-- {\"section\":\"Shared\",\"time\":\"1m\"} -->\n# Included\n",
        )
        .unwrap();
        let source = "<!-- {\"include\":\"shared.md\"} -->\n---\n# After\n";

        let expanded = expand_includes(source, 0, &deck).unwrap();
        let frontmatter = crate::parser::parse_frontmatter(&expanded.source).unwrap();
        let parsed = crate::parser::parse_markdown(
            &expanded.source,
            frontmatter,
            &crate::highlight::Highlighter::defaults(),
        )
        .unwrap();

        assert_eq!(parsed.parsed_slides().len(), 2);
        assert_eq!(parsed.settings().sections()[0].name(), "Shared");
        assert_eq!(parsed.settings().sections()[0].start(), 0);
        assert_eq!(parsed.settings().sections()[0].end(), 1);
    }

    #[test]
    fn included_explicit_body_slot_survives_expand_parse_and_map() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        fs::write(
            dir.path().join("shared.md"),
            "# Included title\n\n::: {slot=body}\n\nIncluded body content\n\n:::\n",
        )
        .unwrap();
        let source = "# Deck slide\n\n---\n<!-- {\"include\":\"shared.md\"} -->\n";
        let top_frontmatter = crate::parser::parse_frontmatter(source).unwrap();

        let expanded = expand_includes(source, top_frontmatter.body_start(), &deck).unwrap();

        assert_eq!(
            expanded.source,
            "# Deck slide\n\n---\n# Included title\n\n::: {slot=body}\n\nIncluded body content\n\n:::\n"
        );

        let frontmatter = crate::parser::parse_frontmatter(&expanded.source).unwrap();
        let parsed = crate::parser::parse_markdown(
            &expanded.source,
            frontmatter,
            &crate::highlight::Highlighter::defaults(),
        )
        .unwrap();

        assert_eq!(parsed.parsed_slides().len(), 2);
        let included = &parsed.parsed_slides()[1];
        let slot_group = included
            .fragments
            .iter()
            .find(|fragment| matches!(fragment.kind(), FragmentKind::SlotGroup { .. }))
            .expect("included slide should keep its explicit slot group");
        let FragmentKind::SlotGroup { name, children } = slot_group.kind() else {
            unreachable!();
        };
        assert_eq!(name.as_slot_name().as_str(), "body");
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].kind(), &FragmentKind::Paragraph);
        assert_eq!(children[0].markdown(), "Included body content");

        let layout = crate::layout::parse_layout(
            "title-body",
            r#"<section>
                <slot name="title" accepts="inline" arity="1"></slot>
                <slot name="body" accepts="blocks" arity="0..*"></slot>
            </section>"#,
        )
        .unwrap();
        let mapped = crate::mapping::map_by_convention(parsed, &layout).unwrap();
        let body_slot = SlotName::new("body").unwrap();
        let body_fragments = mapped.mapped_slides()[1].slots[&body_slot].fragments();

        assert_eq!(body_fragments.len(), 1);
        assert_eq!(body_fragments[0].kind(), &FragmentKind::Paragraph);
        assert_eq!(body_fragments[0].markdown(), "Included body content");
    }

    #[test]
    fn nested_include_chain_splices_all_slides_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        fs::write(
            dir.path().join("a.md"),
            "# A\n\n---\n<!-- {\"include\":\"b.md\"} -->\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("b.md"),
            "# B\n\n---\n<!-- {\"include\":\"c.md\"} -->\n",
        )
        .unwrap();
        fs::write(dir.path().join("c.md"), "# C\n").unwrap();
        let source = "<!-- {\"include\":\"a.md\"} -->\n";

        let expanded = expand_includes(source, 0, &deck).unwrap();

        assert_eq!(expanded.source, "# A\n\n---\n# B\n\n---\n# C\n");
        assert_eq!(expanded.line_map.translate(7), (dir.path().join("c.md"), 1));
    }

    #[test]
    fn acyclic_include_chain_over_max_depth_is_a_line_numbered_error() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        for index in 1..=70 {
            let path = dir.path().join(format!("{index}.md"));
            let source = if index == 70 {
                "# Leaf\n".to_owned()
            } else {
                format!("\n<!-- {{\"include\":\"{}.md\"}} -->\n", index + 1)
            };
            fs::write(path, source).unwrap();
        }
        let source = "\n<!-- {\"include\":\"1.md\"} -->\n";

        let err = expand_includes(source, 0, &deck).unwrap_err();

        assert_eq!(err.line, Some(2));
        assert_eq!(err.message, "include chain exceeds max depth of 64");
        assert_eq!(err.help, "reduce include nesting");
    }

    #[test]
    fn self_include_cycle_is_a_line_numbered_error() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        fs::write(
            dir.path().join("shared.md"),
            "<!-- {\"include\":\"shared.md\"} -->\n",
        )
        .unwrap();
        let source = "<!-- {\"include\":\"shared.md\"} -->\n";

        let err = expand_includes(source, 0, &deck).unwrap_err();

        assert_eq!(err.line, Some(1));
        assert!(err.message.contains("include cycle detected"));
        assert!(err.message.contains("shared.md -> shared.md"));
    }

    #[test]
    fn multi_file_include_cycle_is_a_line_numbered_error() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("deck.md");
        fs::write(dir.path().join("a.md"), "<!-- {\"include\":\"b.md\"} -->\n").unwrap();
        fs::write(dir.path().join("b.md"), "<!-- {\"include\":\"a.md\"} -->\n").unwrap();
        let source = "<!-- {\"include\":\"a.md\"} -->\n";

        let err = expand_includes(source, 0, &deck).unwrap_err();

        assert_eq!(err.line, Some(1));
        assert!(err.message.contains("include cycle detected"));
        assert!(err.message.contains("a.md -> b.md -> a.md"));
    }
}
