// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Line-based three-way text merge engine (diff3 style).
//!
//! Given an ancestor and two derived versions (left and right), produces a
//! sequence of [`MergeSection`]s that classify each region of the merge as
//! unchanged, changed on one side only, changed identically on both sides,
//! or conflicting. When no ancestor is available, a two-way merge can be
//! performed instead, where any difference between the sides is reported as
//! a conflict.

/// How a section of a merge came about.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MergeKind {
    /// All inputs agree on these lines.
    Same,
    /// Only the left side differs from the ancestor.
    LeftChanged,
    /// Only the right side differs from the ancestor.
    RightChanged,
    /// Both sides differ from the ancestor in the same way.
    BothSame,
    /// The sides differ from each other and cannot be auto-merged.
    Conflict,
}

/// A contiguous region of a merge with its per-side contents.
///
/// Concatenating the `ancestor`, `left`, and `right` contents of all
/// sections of a [`MergeResult`] reproduces the respective input texts.
#[derive(Clone, Debug)]
pub struct MergeSection {
    kind: MergeKind,
    ancestor: Vec<String>,
    left: Vec<String>,
    right: Vec<String>,
}

impl MergeSection {
    /// The classification of this section.
    pub fn kind(&self) -> MergeKind {
        self.kind
    }

    /// The ancestor lines covered by this section (empty in two-way mode).
    pub fn ancestor(&self) -> &[String] {
        &self.ancestor
    }

    /// The left-side lines for this section.
    pub fn left(&self) -> &[String] {
        &self.left
    }

    /// The right-side lines for this section.
    pub fn right(&self) -> &[String] {
        &self.right
    }

    /// Whether this section is an unresolved conflict.
    pub fn is_conflict(&self) -> bool {
        self.kind == MergeKind::Conflict
    }
}

/// The result of a two- or three-way merge.
#[derive(Clone, Debug, Default)]
pub struct MergeResult {
    sections: Vec<MergeSection>,
}

impl MergeResult {
    /// The merge sections in document order.
    pub fn sections(&self) -> &[MergeSection] {
        &self.sections
    }

    /// The number of conflicting sections.
    pub fn conflict_count(&self) -> usize {
        self.sections
            .iter()
            .filter(|s| s.kind == MergeKind::Conflict)
            .count()
    }

    /// The indices of all conflicting sections.
    pub fn conflict_indices(&self) -> Vec<usize> {
        self.sections
            .iter()
            .enumerate()
            .filter(|(_, s)| s.kind == MergeKind::Conflict)
            .map(|(i, _)| i)
            .collect()
    }

    /// The output text constructed from the merge without user input.
    ///
    /// Unchanged and one-sided sections contribute their resolved lines;
    /// conflicts tentatively contribute the left side.
    pub fn auto_output(&self) -> Vec<String> {
        self.sections
            .iter()
            .flat_map(|s| match s.kind {
                MergeKind::Same
                | MergeKind::BothSame
                | MergeKind::LeftChanged
                | MergeKind::Conflict => s.left.iter(),
                MergeKind::RightChanged => s.right.iter(),
            })
            .cloned()
            .collect()
    }
}

/// An aligned chunk between an anchor text and a derived side.
#[derive(Clone, Copy, Debug)]
struct Chunk {
    /// Start offset in the anchor text.
    a_start: usize,
    /// Length in anchor lines (zero for pure insertions).
    a_len: usize,
    /// Start offset in the side text.
    s_start: usize,
    /// Length in side lines (zero for pure deletions).
    s_len: usize,
    /// Whether the side differs from the anchor in this chunk.
    changed: bool,
}

/// A span of side lines anchored at a position in the ancestor.
#[derive(Clone, Copy, Debug)]
struct Span {
    anc_start: usize,
    anc_len: usize,
    start: usize,
    len: usize,
    changed: bool,
}

/// A merged region of the ancestor touched by at least one side.
#[derive(Clone, Copy, Debug)]
struct Region {
    start: usize,
    end: usize,
}

/// Diff the anchor against a side and produce aligned chunks.
fn diff_chunks(a: &[&str], b: &[&str]) -> Vec<Chunk> {
    let changes = diff::slice(a, b);
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut ap = 0usize;
    let mut bp = 0usize;
    for change in changes {
        let (al, bl, changed) = match change {
            diff::Result::Both(..) => (1, 1, false),
            diff::Result::Left(..) => (1, 0, true),
            diff::Result::Right(..) => (0, 1, true),
        };
        // Extend the previous chunk when the kind matches and both sides
        // are contiguous, so runs of equal or changed lines collapse.
        match chunks.last_mut() {
            Some(last)
                if last.changed == changed
                    && last.a_start + last.a_len == ap
                    && last.s_start + last.s_len == bp =>
            {
                last.a_len += al;
                last.s_len += bl;
            }
            _ => chunks.push(Chunk {
                a_start: ap,
                a_len: al,
                s_start: bp,
                s_len: bl,
                changed,
            }),
        }
        ap += al;
        bp += bl;
    }
    chunks
}

/// Merge per-side hunks into ancestor regions.
///
/// Overlapping hunks are joined. Two point insertions at the same ancestor
/// position are joined as well, but a point insertion that merely touches
/// the boundary of a modification stays a separate region, so insertions at
/// modification boundaries are not swallowed by the modification.
fn merge_regions(hunks: &[(usize, usize)]) -> Vec<Region> {
    let mut sorted = hunks.to_vec();
    sorted.sort_unstable();
    let mut regions: Vec<Region> = Vec::new();
    for (start, end) in sorted {
        if let Some(last) = regions.last_mut() {
            let join = start < last.end
                || (start == last.end && start == last.start && end == start);
            if join {
                last.end = last.end.max(end);
                continue;
            }
        }
        regions.push(Region { start, end });
    }
    regions
}

/// Collect the side lines and changed flags for every region.
///
/// Spans with anchor length may straddle a region boundary (e.g. an equal
/// run that covers several regions), so their intersection with the region
/// is taken; point insertions belong wholly to one region.
fn region_contents(
    spans: &[Span],
    regions: &[Region],
    lines: &[&str],
) -> (Vec<Vec<String>>, Vec<bool>) {
    let mut contents = vec![Vec::new(); regions.len()];
    let mut changed = vec![false; regions.len()];
    for (i, region) in regions.iter().enumerate() {
        if region.start == region.end {
            // Point region: only insertions anchored exactly here.
            let lo = spans.partition_point(|s| s.anc_start < region.start);
            let hi = spans.partition_point(|s| s.anc_start < region.start + 1);
            for span in &spans[lo..hi] {
                if span.anc_len == 0 {
                    contents[i].extend(
                        lines[span.start..span.start + span.len]
                            .iter()
                            .map(|l| (*l).to_string()),
                    );
                    if span.changed {
                        changed[i] = true;
                    }
                }
            }
            continue;
        }
        // Spans are sorted by anchor position and tile the anchor, so
        // `anc_start + anc_len` is non-decreasing and partition_point
        // applies to both bounds.
        let lo =
            spans.partition_point(|s| s.anc_start + s.anc_len <= region.start);
        let hi = spans.partition_point(|s| s.anc_start < region.end);
        for span in &spans[lo..hi] {
            if span.changed {
                // A changed chunk is an atomic replacement block: its side
                // lines belong wholly to the region that contains it.
                let overlaps = if span.anc_len == 0 {
                    span.anc_start > region.start
                        && span.anc_start < region.end
                } else {
                    span.anc_start < region.end
                        && span.anc_start + span.anc_len > region.start
                };
                if overlaps {
                    contents[i].extend(
                        lines[span.start..span.start + span.len]
                            .iter()
                            .map(|l| (*l).to_string()),
                    );
                    changed[i] = true;
                }
                continue;
            }
            // An equal run may straddle the region; take the intersection.
            let is = span.anc_start.max(region.start);
            let ie = (span.anc_start + span.anc_len).min(region.end);
            if is < ie {
                let off = span.start + (is - span.anc_start);
                contents[i].extend(
                    lines[off..off + (ie - is)]
                        .iter()
                        .map(|l| (*l).to_string()),
                );
            }
        }
    }
    (contents, changed)
}

/// Assemble the final sections from the regions and their contents.
fn build_result(
    anchor: &[&str],
    regions: &[Region],
    lcontent: Vec<Vec<String>>,
    rcontent: Vec<Vec<String>>,
    lchanged: Vec<bool>,
    rchanged: Vec<bool>,
    three_way: bool,
) -> MergeResult {
    let mut sections: Vec<MergeSection> = Vec::new();
    let mut pos = 0usize;
    for (i, region) in regions.iter().enumerate() {
        if region.start > pos {
            // Unchanged lines between the previous region and this one.
            let lines: Vec<String> = anchor[pos..region.start]
                .iter()
                .map(|l| (*l).to_string())
                .collect();
            sections.push(MergeSection {
                kind: MergeKind::Same,
                ancestor: lines.clone(),
                left: lines.clone(),
                right: lines,
            });
        }
        let kind = if three_way {
            match (lchanged[i], rchanged[i]) {
                (true, true) => {
                    if lcontent[i] == rcontent[i] {
                        MergeKind::BothSame
                    } else {
                        MergeKind::Conflict
                    }
                }
                (true, false) => MergeKind::LeftChanged,
                (false, true) => MergeKind::RightChanged,
                (false, false) => MergeKind::Same,
            }
        } else {
            // Without an ancestor any difference is a conflict.
            if lchanged[i] || rchanged[i] {
                MergeKind::Conflict
            } else {
                MergeKind::Same
            }
        };
        // In two-way mode there is no ancestor text to report.
        let ancestor: Vec<String> = if three_way {
            anchor[region.start..region.end]
                .iter()
                .map(|l| (*l).to_string())
                .collect()
        } else {
            Vec::new()
        };
        sections.push(MergeSection {
            kind,
            ancestor,
            left: lcontent[i].clone(),
            right: rcontent[i].clone(),
        });
        pos = region.end;
    }
    if pos < anchor.len() {
        let lines: Vec<String> =
            anchor[pos..].iter().map(|l| (*l).to_string()).collect();
        sections.push(MergeSection {
            kind: MergeKind::Same,
            ancestor: lines.clone(),
            left: lines.clone(),
            right: lines,
        });
    }
    MergeResult { sections }
}

/// Merge `left` and `right`, optionally against a common `ancestor`.
///
/// With an ancestor, a three-way (diff3 style) merge is performed: regions
/// changed on only one side are resolved automatically. Without an ancestor
/// (two-way mode), any differing region is reported as a conflict.
pub fn merge_texts(
    ancestor: Option<&str>,
    left: &str,
    right: &str,
) -> MergeResult {
    let left_lines: Vec<&str> = left.lines().collect();
    let right_lines: Vec<&str> = right.lines().collect();
    match ancestor {
        Some(anc) => {
            let anc_lines: Vec<&str> = anc.lines().collect();
            let lchunks = diff_chunks(&anc_lines, &left_lines);
            let rchunks = diff_chunks(&anc_lines, &right_lines);
            let lspans: Vec<Span> = lchunks
                .iter()
                .map(|c| Span {
                    anc_start: c.a_start,
                    anc_len: c.a_len,
                    start: c.s_start,
                    len: c.s_len,
                    changed: c.changed,
                })
                .collect();
            let rspans: Vec<Span> = rchunks
                .iter()
                .map(|c| Span {
                    anc_start: c.a_start,
                    anc_len: c.a_len,
                    start: c.s_start,
                    len: c.s_len,
                    changed: c.changed,
                })
                .collect();
            let mut hunks: Vec<(usize, usize)> = Vec::new();
            for chunks in [&lchunks, &rchunks] {
                for c in chunks.iter().filter(|c| c.changed) {
                    hunks.push((c.a_start, c.a_start + c.a_len));
                }
            }
            let regions = merge_regions(&hunks);
            let (lcontent, lchanged) =
                region_contents(&lspans, &regions, &left_lines);
            let (rcontent, rchanged) =
                region_contents(&rspans, &regions, &right_lines);
            build_result(
                &anc_lines, &regions, lcontent, rcontent, lchanged, rchanged,
                true,
            )
        }
        None => {
            // Two-way mode: align left against right. The anchor coordinate
            // space is the left text; right-only insertions are point spans.
            let chunks = diff_chunks(&left_lines, &right_lines);
            let lspans: Vec<Span> = chunks
                .iter()
                .filter(|c| c.a_len != 0)
                .map(|c| Span {
                    anc_start: c.a_start,
                    anc_len: c.a_len,
                    start: c.a_start,
                    len: c.a_len,
                    changed: c.changed,
                })
                .collect();
            let rspans: Vec<Span> = chunks
                .iter()
                .filter(|c| c.s_len != 0)
                .map(|c| Span {
                    anc_start: c.a_start,
                    anc_len: c.a_len,
                    start: c.s_start,
                    len: c.s_len,
                    changed: c.changed,
                })
                .collect();
            let hunks: Vec<(usize, usize)> = chunks
                .iter()
                .filter(|c| c.changed)
                .map(|c| (c.a_start, c.a_start + c.a_len))
                .collect();
            let regions = merge_regions(&hunks);
            let (lcontent, lchanged) =
                region_contents(&lspans, &regions, &left_lines);
            let (rcontent, rchanged) =
                region_contents(&rspans, &regions, &right_lines);
            build_result(
                &left_lines,
                &regions,
                lcontent,
                rcontent,
                lchanged,
                rchanged,
                false,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    /// Assert that concatenating each side's section contents reproduces the
    /// respective input text.
    fn assert_concatenation(
        result: &MergeResult,
        anc: &str,
        left: &str,
        right: &str,
    ) {
        let join = |parts: Vec<&[String]>| -> Vec<String> { parts.concat() };
        assert_eq!(
            join(result.sections().iter().map(|s| s.left()).collect()),
            lines(left),
            "left concatenation mismatch"
        );
        assert_eq!(
            join(result.sections().iter().map(|s| s.right()).collect()),
            lines(right),
            "right concatenation mismatch"
        );
        if !anc.is_empty() {
            assert_eq!(
                join(result.sections().iter().map(|s| s.ancestor()).collect()),
                lines(anc),
                "ancestor concatenation mismatch"
            );
        }
    }

    #[test]
    fn identical_inputs_are_one_same_section() {
        let text = "a\nb\nc\n";
        let result = merge_texts(Some(text), text, text);
        assert_eq!(result.sections().len(), 1);
        assert_eq!(result.sections()[0].kind(), MergeKind::Same);
        assert_eq!(result.conflict_count(), 0);
        assert_eq!(result.auto_output(), lines(text));
    }

    #[test]
    fn left_only_change_is_auto_resolved() {
        let result = merge_texts(Some("a\nb\nc\n"), "a\nB\nc\n", "a\nb\nc\n");
        assert_eq!(result.conflict_count(), 0);
        let kinds: Vec<MergeKind> =
            result.sections().iter().map(|s| s.kind()).collect();
        assert!(kinds.contains(&MergeKind::LeftChanged));
        assert_eq!(result.auto_output(), lines("a\nB\nc\n"));
        assert_concatenation(&result, "a\nb\nc\n", "a\nB\nc\n", "a\nb\nc\n");
    }

    #[test]
    fn right_only_change_is_auto_resolved() {
        let result = merge_texts(Some("a\nb\nc\n"), "a\nb\nc\n", "a\nB\nc\n");
        assert_eq!(result.conflict_count(), 0);
        assert_eq!(result.auto_output(), lines("a\nB\nc\n"));
    }

    #[test]
    fn both_same_change_is_not_a_conflict() {
        let result = merge_texts(Some("a\nb\nc\n"), "a\nB\nc\n", "a\nB\nc\n");
        assert_eq!(result.conflict_count(), 0);
        let kinds: Vec<MergeKind> =
            result.sections().iter().map(|s| s.kind()).collect();
        assert!(kinds.contains(&MergeKind::BothSame));
        // The identical change appears exactly once in the output.
        assert_eq!(result.auto_output(), lines("a\nB\nc\n"));
    }

    #[test]
    fn opposing_changes_conflict() {
        let result = merge_texts(Some("a\nb\nc\n"), "a\nL\nc\n", "a\nR\nc\n");
        assert_eq!(result.conflict_count(), 1);
        let conflict = &result.sections()[result.conflict_indices()[0]];
        assert_eq!(conflict.left(), ["L".to_string()]);
        assert_eq!(conflict.right(), ["R".to_string()]);
        assert_eq!(conflict.ancestor(), ["b".to_string()]);
        // Conflicts tentatively take the left side.
        assert_eq!(result.auto_output(), lines("a\nL\nc\n"));
    }

    #[test]
    fn distant_changes_merge_cleanly() {
        let anc = "1\n2\n3\n4\n5\n6\n7\n8\n";
        let left = "1x\n2\n3\n4\n5\n6\n7\n8\n";
        let right = "1\n2\n3\n4\n5\n6\n7\n8x\n";
        let result = merge_texts(Some(anc), left, right);
        assert_eq!(result.conflict_count(), 0);
        assert_eq!(result.auto_output(), lines("1x\n2\n3\n4\n5\n6\n7\n8x\n"));
        assert_concatenation(&result, anc, left, right);
    }

    #[test]
    fn same_insertion_at_same_point_is_both_same() {
        let result = merge_texts(Some("a\nb\n"), "a\nX\nb\n", "a\nX\nb\n");
        assert_eq!(result.conflict_count(), 0);
        assert_eq!(result.auto_output(), lines("a\nX\nb\n"));
    }

    #[test]
    fn different_insertions_at_same_point_conflict() {
        let result = merge_texts(Some("a\nb\n"), "a\nL\nb\n", "a\nR\nb\n");
        assert_eq!(result.conflict_count(), 1);
        assert_eq!(result.auto_output(), lines("a\nL\nb\n"));
        assert_concatenation(&result, "a\nb\n", "a\nL\nb\n", "a\nR\nb\n");
    }

    #[test]
    fn insertion_at_modification_boundary_stays_separate() {
        // Left modifies ancestor line 2, right inserts after line 2. The
        // insertion must survive as its own section, not be swallowed.
        let anc = "1\n2\n3\n";
        let left = "1\n2L\n3\n";
        let right = "1\n2\nR\n3\n";
        let result = merge_texts(Some(anc), left, right);
        assert_eq!(result.conflict_count(), 0);
        assert_eq!(result.auto_output(), lines("1\n2L\nR\n3\n"));
        assert_concatenation(&result, anc, left, right);
    }

    #[test]
    fn insertion_inside_other_modification_conflicts() {
        let anc = "1\n2\n3\n4\n";
        let left = "1\n2L\n3L\n4\n";
        let right = "1\n2\nX\n3\n4\n";
        let result = merge_texts(Some(anc), left, right);
        assert_eq!(result.conflict_count(), 1);
        assert_concatenation(&result, anc, left, right);
    }

    #[test]
    fn deletions_on_both_sides_of_same_region_conflict() {
        let result = merge_texts(Some("a\nb\nc\n"), "a\nc\n", "a\nb\nc\n");
        // Left deleted `b`, right kept it: one-sided change, no conflict.
        assert_eq!(result.conflict_count(), 0);
        assert_eq!(result.auto_output(), lines("a\nc\n"));
    }

    #[test]
    fn opposing_deletions_and_edits_conflict() {
        let result = merge_texts(Some("a\nb\nc\n"), "a\nc\n", "a\nB\nc\n");
        assert_eq!(result.conflict_count(), 1);
    }

    #[test]
    fn empty_ancestor_treats_differences_as_conflicts() {
        let result = merge_texts(Some(""), "a\n", "b\n");
        assert_eq!(result.conflict_count(), 1);
        let result = merge_texts(Some(""), "a\n", "a\n");
        assert_eq!(result.conflict_count(), 0);
        assert_eq!(result.auto_output(), lines("a\n"));
    }

    #[test]
    fn two_way_mode_splits_equal_and_conflicting() {
        let result = merge_texts(None, "a\nL\nc\n", "a\nR\nc\n");
        let kinds: Vec<MergeKind> =
            result.sections().iter().map(|s| s.kind()).collect();
        assert_eq!(
            kinds,
            vec![MergeKind::Same, MergeKind::Conflict, MergeKind::Same,]
        );
        assert_eq!(result.conflict_count(), 1);
        assert_eq!(result.auto_output(), lines("a\nL\nc\n"));
    }

    #[test]
    fn two_way_mode_insertion_is_conflict() {
        let result = merge_texts(None, "a\nb\n", "a\nX\nb\n");
        assert_eq!(result.conflict_count(), 1);
        assert_concatenation(&result, "", "a\nb\n", "a\nX\nb\n");
    }

    #[test]
    fn two_way_identical_has_no_conflicts() {
        let result = merge_texts(None, "a\nb\n", "a\nb\n");
        assert_eq!(result.conflict_count(), 0);
        assert_eq!(result.auto_output(), lines("a\nb\n"));
    }

    #[test]
    fn empty_sides_are_handled() {
        let result = merge_texts(Some("a\n"), "", "a\n");
        assert_eq!(result.conflict_count(), 0);
        assert_eq!(result.auto_output(), Vec::<String>::new());
        let result = merge_texts(Some("a\n"), "a\n", "");
        assert_eq!(result.conflict_count(), 0);
        assert_eq!(result.auto_output(), Vec::<String>::new());
        let result = merge_texts(Some(""), "", "");
        assert!(result.sections().is_empty());
        assert_eq!(result.auto_output(), Vec::<String>::new());
    }

    #[test]
    fn multi_hunk_conflicts_are_counted() {
        let anc = "1\n2\n3\n4\n5\n6\n7\n8\n";
        let left = "1L\n2\n3\n4\n5L\n6\n7\n8\n";
        let right = "1R\n2\n3\n4\n5R\n6\n7\n8\n";
        let result = merge_texts(Some(anc), left, right);
        assert_eq!(result.conflict_count(), 2);
        assert_concatenation(&result, anc, left, right);
    }
}
