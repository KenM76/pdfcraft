//! Reading order for auto-tagging: a page set in columns is read down each column, left to right.
//!
//! A gutter is a vertical band, at least [`GUTTER_EM`] wide, that at most a fifth of the page's
//! text lines cross ([`LOW_SHARE`]), with a column beside it on each side. A column needs
//! [`MIN_COLUMN_LINES`] lines, a width of [`MIN_COLUMN_EM`], and must sit beside its neighbour for
//! at least half the shorter one's height ([`BESIDE_SHARE`]); a gutter that fails is dropped and the
//! rest are checked again. Text that crosses a gutter, such as a title over both columns, splits the
//! page into bands and keeps its place between them. A page with no gutter, with any text whose
//! position is not known, or with an unknown body size keeps content order.

use crate::autotag_roles::Evidence;
use crate::autotag_tables::Candidate;

/// Gutter width in body ems: wider than any word space (about a quarter to a third of an em), so
/// the gaps between words drawn as separate objects never line up into one.
const GUTTER_EM: f64 = 1.0;
/// The most lines that may cross a gutter, as a share of the lines at the busiest x: room for a
/// title, a caption or a rule of text over the columns, but not for a paragraph's ragged edge.
const LOW_SHARE: i64 = 5;
/// A column holds at least this many lines; fewer is a label or a note, read in content order.
const MIN_COLUMN_LINES: i64 = 3;
/// A column is at least this many body ems wide: a few words a line. Narrower runs are lists of
/// labels or numbers whose content order is as good as any.
const MIN_COLUMN_EM: f64 = 8.0;
/// Two neighbouring columns overlap vertically by at least this share of the shorter one, so the
/// gutter runs most of the text's height rather than being the gap beside one indented block.
const BESIDE_SHARE: f64 = 0.5;
/// More candidate gutters than this is not a page of columns but scattered labels, as on a chart;
/// it also bounds the work of checking them.
const MAX_GUTTERS: usize = 8;

/// What is ordered: one text object, or every cell of one table, which is written as a whole.
struct Unit {
    members: Vec<usize>,
    x0: f64,
    x1: f64,
    /// Lowest and highest baseline.
    low: f64,
    high: f64,
    lines: i64,
}

/// The page's text objects, by index, in reading order; `None` keeps content order.
pub(crate) fn reading_order(objects: &[Evidence], tables: &[Candidate]) -> Option<Vec<usize>> {
    let units = units(objects, tables)?;
    let em = body_size(objects)?;
    let columns = columns(&units, em)?;
    let tolerance = em / 2.0;
    let column_of = |u: &Unit| columns.iter().position(|&(a, b)| u.x0 >= a - tolerance && u.x1 <= b + tolerance);
    let mut spanners: Vec<f64> = units.iter().filter(|u| column_of(u).is_none()).map(|u| u.high).collect();
    spanners.sort_by(|a, b| b.total_cmp(a));
    let mut keyed: Vec<((usize, u8, usize), &Unit)> = units
        .iter()
        .map(|u| {
            let band = spanners.iter().filter(|&&s| s > u.high).count();
            let key = match column_of(u) {
                Some(c) => (band, 0, c),
                None => (band, 1, 0),
            };
            (key, u)
        })
        .collect();
    keyed.sort_by(|(ka, a), (kb, b)| ka.cmp(kb).then(b.high.total_cmp(&a.high)).then(a.x0.total_cmp(&b.x0)));
    Some(keyed.into_iter().flat_map(|(_, u)| u.members.iter().copied()).collect())
}

/// Whether `order` lists each of `0..len` once.
pub(crate) fn is_permutation(order: &[usize], len: usize) -> bool {
    let mut seen = vec![false; len];
    order.len() == len && order.iter().all(|&i| seen.get_mut(i).is_some_and(|s| !std::mem::replace(s, true)))
}

/// Tables as single units, each other placed object as one, and objects with no text joined to the
/// unit before them. `None` when text sits where it cannot be placed.
fn units(objects: &[Evidence], tables: &[Candidate]) -> Option<Vec<Unit>> {
    let mut units: Vec<Unit> = Vec::new();
    let mut owner = vec![None; objects.len()];
    for (t, candidate) in tables.iter().enumerate() {
        if let Ok(grid) = &candidate.outcome {
            for &i in grid.rows.iter().flatten().flatten() {
                if let Some(slot) = owner.get_mut(i) {
                    *slot = Some(t);
                }
            }
        }
    }
    let mut table_unit = vec![None; tables.len()];
    let mut leading = Vec::new();
    for (i, e) in objects.iter().enumerate() {
        if let Some(t) = owner.get(i).copied().flatten() {
            let at = match table_unit.get(t).copied().flatten() {
                Some(at) => at,
                None => {
                    let rect = tables.get(t)?.rect;
                    let [x0, low, x1, high] = Some(rect).filter(|r| r.iter().all(|v| v.is_finite()))?;
                    let lines = i64::try_from(tables.get(t)?.rows).unwrap_or(i64::MAX);
                    units.push(Unit { members: Vec::new(), x0, x1, low, high, lines });
                    *table_unit.get_mut(t)? = Some(units.len() - 1);
                    units.len() - 1
                }
            };
            units.get_mut(at)?.members.push(i);
        } else if e.chars == 0 && !e.unmapped {
            match units.last_mut() {
                Some(u) => u.members.push(i),
                None => leading.push(i),
            }
        } else {
            let [x0, low, x1, high] = e.place.filter(|p| p.iter().all(|v| v.is_finite()))?;
            let lines = i64::try_from(e.lines.max(1)).unwrap_or(i64::MAX);
            units.push(Unit { members: vec![i], x0, x1, low, high, lines });
        }
    }
    if let Some(first) = units.first_mut() {
        leading.append(&mut first.members);
        first.members = leading;
    }
    Some(units)
}

/// The size carrying the median character: the body text's em.
fn body_size(objects: &[Evidence]) -> Option<f64> {
    let mut sizes: Vec<(f64, usize)> = objects.iter().filter(|e| e.chars > 0 && e.size > 0.0).map(|e| (e.size, e.chars)).collect();
    sizes.sort_by(|a, b| a.0.total_cmp(&b.0));
    let half = sizes.iter().fold(0usize, |sum, s| sum.saturating_add(s.1)) / 2;
    let mut seen = 0usize;
    sizes
        .into_iter()
        .find(|s| {
            seen = seen.saturating_add(s.1);
            seen > half
        })
        .map(|s| s.0)
        .filter(|s| s.is_finite())
}

/// The columns' x-intervals, left to right, when there are two or more.
fn columns(units: &[Unit], em: f64) -> Option<Vec<(f64, f64)>> {
    let mut gutters = gutters(units, em);
    if gutters.len() > MAX_GUTTERS {
        return None;
    }
    while !gutters.is_empty() {
        let left = units.iter().map(|u| u.x0).fold(f64::INFINITY, f64::min);
        let right = units.iter().map(|u| u.x1).fold(f64::NEG_INFINITY, f64::max);
        let mut columns = Vec::with_capacity(gutters.len() + 1);
        let mut start = left;
        for &(a, b) in &gutters {
            columns.push((start, a));
            start = b;
        }
        columns.push((start, right));
        let tolerance = em / 2.0;
        let held: Vec<Vec<&Unit>> =
            columns.iter().map(|&(a, b)| units.iter().filter(|u| u.x0 >= a - tolerance && u.x1 <= b + tolerance).collect()).collect();
        let weak = held.iter().zip(&columns).position(|(inside, &(a, b))| {
            b - a < MIN_COLUMN_EM * em || inside.iter().fold(0i64, |n, u| n.saturating_add(u.lines)) < MIN_COLUMN_LINES
        });
        let apart = held.windows(2).position(|pair| match pair {
            [l, r] => !beside(l, r),
            _ => false,
        });
        match weak.map(|c| c.min(gutters.len() - 1)).or(apart) {
            Some(g) if g < gutters.len() => {
                gutters.remove(g);
            }
            _ => return Some(columns),
        }
    }
    None
}

/// Whether two columns overlap vertically for at least [`BESIDE_SHARE`] of the shorter one.
fn beside(left: &[&Unit], right: &[&Unit]) -> bool {
    let extent = |units: &[&Unit]| {
        let low = units.iter().map(|u| u.low).fold(f64::INFINITY, f64::min);
        let high = units.iter().map(|u| u.high).fold(f64::NEG_INFINITY, f64::max);
        (low, high)
    };
    let ((l0, l1), (r0, r1)) = (extent(left), extent(right));
    let shorter = (l1 - l0).min(r1 - r0);
    let shared = l1.min(r1) - l0.max(r0);
    shared.is_finite() && shared > 0.0 && shared >= BESIDE_SHARE * shorter
}

/// The x-bands strictly inside the text that few enough lines cross, at least [`GUTTER_EM`] wide.
fn gutters(units: &[Unit], em: f64) -> Vec<(f64, f64)> {
    let mut edges: Vec<(f64, i64)> = units.iter().flat_map(|u| [(u.x0, u.lines), (u.x1, -u.lines)]).collect();
    // Ends before starts at one x, so objects that only touch leave a zero-width gap, not an overlap.
    edges.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut level = 0i64;
    let mut runs: Vec<(f64, f64, i64)> = Vec::with_capacity(edges.len());
    for pair in edges.windows(2) {
        if let [(x, change), (next, _)] = pair {
            level = level.saturating_add(*change);
            runs.push((*x, *next, level));
        }
    }
    let peak = runs.iter().map(|r| r.2).max().unwrap_or(0);
    let low = peak / LOW_SHARE;
    let mut out: Vec<(f64, f64)> = Vec::new();
    // A band open from the left edge has no text on its left, and one still open at the right
    // edge none on its right: neither is a gutter.
    let (mut open, mut text_left): (Option<(f64, f64)>, bool) = (None, false);
    for &(a, b, level) in &runs {
        if level <= low {
            open = Some(open.map_or((a, b), |(start, _)| (start, b)));
            continue;
        }
        if let Some((start, end)) = open.take()
            && text_left
            && end - start >= GUTTER_EM * em
        {
            out.push((start, end));
        }
        text_left = true;
    }
    out
}

#[cfg(test)]
mod tests {
    use pdfcraft_cos::{Document, Object};

    use crate::tests::{pdf, stream};
    use crate::{autotag_text, tag_content, tags};

    /// One text object at `(x, y)` in Helvetica (`F1`) or Helvetica-Bold (`F2`).
    fn line(font: &str, size: f64, x: f64, y: f64, text: &str) -> String {
        format!("BT /{font} {size} Tf {x} {y} Td ({text}) Tj ET ")
    }

    /// A one-page document drawing `content`, with `extra` in the page dictionary, in Helvetica
    /// (`F1`), Helvetica-Bold (`F2`) and a two-byte font with no Unicode mapping (`F3`).
    fn page_with(content: &str, extra: &str) -> Document {
        pdf(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>".into(),
                "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
                format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R {extra} /Resources << /Font << /F1 5 0 R /F2 6 0 R /F3 7 0 R >> >> >>"),
                stream(content),
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>".into(),
                "<< /Type /Font /Subtype /Type0 /BaseFont /House /Encoding /Identity-H /DescendantFonts [8 0 R] >>".into(),
                "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /House /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /DW 1000 >>".into(),
            ],
            "",
        )
    }

    fn page(content: &str) -> Document {
        page_with(content, "")
    }

    /// The tagged text in the order the structure tree reads it.
    fn read(content: &str) -> Vec<String> {
        read_with(content, "")
    }

    fn read_with(content: &str, extra: &str) -> Vec<String> {
        let mut doc = page_with(content, extra);
        autotag_text(&mut doc).unwrap();
        tags(&doc)
            .into_iter()
            .filter(|t| t.kind != "Document")
            .map(|t| tag_content(&doc, t.object).unwrap().into_iter().map(|c| c.text).collect::<String>())
            .collect()
    }

    /// The marked-content IDs each parent-tree entry's element owns, in parent-tree order.
    fn parent_tree_owners(doc: &Document) -> Vec<i64> {
        let follow = |o: Option<&Object>| doc.resolve(o.unwrap()).as_dict().unwrap().clone();
        let catalog = doc.get(doc.root().unwrap()).as_dict().unwrap().clone();
        let tree = follow(follow(catalog.get(b"StructTreeRoot")).get(b"ParentTree"));
        let Some(Object::Array(nums)) = tree.get(b"Nums") else { panic!("a number tree") };
        let Some(Object::Array(parents)) = nums.get(1) else { panic!("one page's parents") };
        parents.iter().map(|p| follow(Some(p)).get(b"K").and_then(Object::as_int).unwrap()).collect()
    }

    /// Prose for line `n` of column `name`, about two hundred units wide in 10 pt Helvetica.
    fn prose(name: &str, n: usize) -> String {
        format!("{name} {n} carries a full line of running prose")
    }

    fn column(name: &str, rows: usize) -> Vec<String> {
        (1..=rows).map(|n| prose(name, n)).collect()
    }

    /// Two columns of `rows` lines, drawn row by row: each left line, then the right one beside it.
    fn interleaved(rows: usize) -> String {
        (1..=rows)
            .map(|n| {
                [
                    line("F1", 10.0, 72.0, 720.0 - 14.0 * n as f64, &prose("Left", n)),
                    line("F1", 10.0, 320.0, 720.0 - 14.0 * n as f64, &prose("Right", n)),
                ]
                .concat()
            })
            .collect()
    }

    #[test]
    fn two_columns_drawn_row_by_row_read_down_the_left_column_first() {
        let order = read(&interleaved(6));
        assert_eq!(order, [column("Left", 6), column("Right", 6)].concat());
        // Marked content keeps its content-order IDs, and the parent tree still finds each one's tag.
        let mut doc = page(&interleaved(6));
        autotag_text(&mut doc).unwrap();
        assert_eq!(parent_tree_owners(&doc), (0..12).collect::<Vec<_>>());
    }

    #[test]
    fn a_title_and_a_footnote_across_both_columns_keep_their_places() {
        let title = "A title set right across both of the columns";
        let note = "A closing note that also runs across both of the columns below";
        let content = [line("F1", 10.0, 72.0, 500.0, note), interleaved(10), line("F2", 18.0, 72.0, 740.0, title)].concat();
        let expected = [vec![title.to_string()], column("Left", 10), column("Right", 10), vec![note.to_string()]].concat();
        assert_eq!(read(&content), expected);
    }

    #[test]
    fn a_sidebar_drawn_inside_the_main_text_is_read_after_it() {
        let main = |n: usize| line("F1", 10.0, 72.0, 720.0 - 14.0 * n as f64, &prose("Main", n));
        let side = |n: usize| line("F1", 10.0, 400.0, 692.0 - 14.0 * n as f64, &format!("Side {n} is a short note"));
        let content = [main(1), main(2), side(1), side(2), side(3), main(3), main(4), main(5), main(6)].concat();
        let expected = [column("Main", 6), (1..=3).map(|n| format!("Side {n} is a short note")).collect()].concat();
        assert_eq!(read(&content), expected);
    }

    #[test]
    fn a_single_column_keeps_content_order_even_when_drawn_out_of_place() {
        let ys = [700.0, 658.0, 686.0, 672.0];
        let content: String = ys.iter().enumerate().map(|(n, &y)| line("F1", 10.0, 72.0, y, &prose("Only", n))).collect();
        assert_eq!(read(&content), (0..4).map(|n| prose("Only", n)).collect::<Vec<_>>());
    }

    #[test]
    fn blocks_that_do_not_sit_beside_each_other_keep_content_order() {
        // The lower block is drawn first, and indented: its left edge leaves a gap under the upper
        // block's right edge, but the two never share a height, so the gap is not a gutter.
        let lower: String = (1..=3).map(|n| line("F1", 10.0, 320.0, 600.0 - 14.0 * n as f64, &prose("Lower", n))).collect();
        let upper: String = (1..=3).map(|n| line("F1", 10.0, 72.0, 720.0 - 14.0 * n as f64, &prose("Upper", n))).collect();
        assert_eq!(read(&[lower, upper].concat()), [column("Lower", 3), column("Upper", 3)].concat());
    }

    #[test]
    fn three_columns_read_left_to_right() {
        let at =
            |x: f64, name: &str, n: usize| line("F1", 7.0, x, 720.0 - 10.0 * n as f64, &format!("{name} {n} holds a longer line of running prose"));
        let content: String = (1..=4).map(|n| [at(72.0, "West", n), at(240.0, "Mid", n), at(408.0, "East", n)].concat()).collect();
        let names = |name: &str| (1..=4).map(|n| format!("{name} {n} holds a longer line of running prose")).collect::<Vec<_>>();
        assert_eq!(read(&content), [names("West"), names("Mid"), names("East")].concat());
    }

    #[test]
    fn heading_levels_follow_the_reading_order_and_never_skip() {
        // The right column's larger heading is drawn first; read left column first, the left one's
        // smaller heading comes first and must not open the tree at a second level.
        let heading = |font_size: f64, x: f64, text: &str| line("F2", font_size, x, 720.0, text);
        let content = [heading(18.0, 320.0, "East news"), heading(14.0, 72.0, "West news"), interleaved(6)].concat();
        let mut doc = page(&content);
        autotag_text(&mut doc).unwrap();
        let read: Vec<(String, String)> = tags(&doc)
            .into_iter()
            .filter(|t| t.kind != "Document")
            .map(|t| (t.kind.clone(), tag_content(&doc, t.object).unwrap().into_iter().map(|c| c.text).collect()))
            .collect();
        assert_eq!(read.first().map(|r| r.1.as_str()), Some("West news"), "{read:?}");
        let levels: Vec<u32> = read.iter().filter_map(|(k, _)| k.strip_prefix('H')?.parse().ok()).collect();
        assert_eq!(levels.len(), 2, "{read:?}");
        let mut previous = 0;
        for level in levels {
            assert!(level <= previous + 1, "{read:?}");
            previous = level;
        }
    }

    #[test]
    fn text_without_a_place_or_on_a_rotated_page_keeps_content_order() {
        let expected: Vec<String> = (1..=6).flat_map(|n| [prose("Left", n), prose("Right", n)]).collect();
        assert_eq!(read_with(&interleaved(6), "/Rotate 90"), expected);
        // Glyphs with no Unicode mapping paint where their text is unknown.
        let unplaced = [interleaved(6), "BT /F3 10 Tf 72 400 Td <00410042> Tj ET ".to_string()].concat();
        let order = read(&unplaced);
        assert_eq!(order.get(..12), Some(&expected[..]));
    }

    #[test]
    fn scattered_labels_at_the_object_limit_keep_content_order() {
        use crate::autotag_roles::Evidence;
        // One short label every three ems along a line: each gap between them could be a gutter.
        let labels: Vec<Evidence> = (0..4_096)
            .map(|i| {
                let x = 30.0 * f64::from(i);
                Evidence { size: 10.0, lines: 1, chars: 2, letters: true, place: Some([x, 0.0, x + 11.0, 0.0]), ..Evidence::default() }
            })
            .collect();
        assert_eq!(super::reading_order(&labels, &[]), None);
    }
}
