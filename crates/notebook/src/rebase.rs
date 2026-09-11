use std::ops::Range;

const INFINITY: u32 = 1 << 30;
const MAX_CELLS: usize = 4_000_000;

struct Matrix {
    band: usize,
    values: Vec<u32>,
}

impl Matrix {
    fn get(&self, row: usize, column: usize) -> u32 {
        let Some(column) = column
            .checked_add(self.band)
            .and_then(|at| at.checked_sub(row))
        else {
            return INFINITY;
        };
        let width = self.band * 2 + 1;
        if column >= width {
            return INFINITY;
        }
        self.values
            .get(row * width + column)
            .copied()
            .unwrap_or(INFINITY)
    }

    fn build(before: &[char], after: &[char], band: usize) -> Self {
        let width = band * 2 + 1;
        let mut matrix = Self {
            band,
            values: vec![INFINITY; (before.len() + 1) * width],
        };
        for row in 0..=before.len() {
            for column in row.saturating_sub(band)..=after.len().min(row + band) {
                let mut cost = if row == 0 && column == 0 { 0 } else { INFINITY };
                if row > 0 {
                    cost = cost.min(matrix.get(row - 1, column) + 1);
                }
                if column > 0 {
                    cost = cost.min(matrix.get(row, column - 1) + 1);
                }
                if row > 0 && column > 0 && before[row - 1] == after[column - 1] {
                    cost = cost.min(matrix.get(row - 1, column - 1));
                }
                matrix.values[row * width + column + band - row] = cost;
            }
        }
        matrix
    }
}

/// Requires the same mapping in every minimum insertion/deletion alignment.
pub(crate) fn rebase(before: &str, after: &str, range: Range<u32>) -> Option<Range<u32>> {
    if range.start > range.end {
        return None;
    }
    let mut a: Vec<_> = before.chars().collect();
    let offsets: Vec<_> = std::iter::once(0)
        .chain(a.iter().scan(0_u32, |at, character| {
            *at = at.checked_add(character.len_utf16() as u32)?;
            Some(*at)
        }))
        .collect();
    let local =
        offsets.binary_search(&range.start).ok()?..offsets.binary_search(&range.end).ok()?;
    if before == after {
        return Some(range);
    }
    let mut b: Vec<_> = after.chars().collect();
    let (n, m) = (a.len(), b.len());
    let mut band = n.abs_diff(m).max(1);
    let forward = loop {
        let width = band.checked_mul(2)?.checked_add(1)?;
        if (n + 1).checked_mul(width)? > MAX_CELLS {
            return None;
        }
        let matrix = Matrix::build(&a, &b, band);
        if matrix.get(n, m) <= band as u32 {
            break matrix;
        }
        band *= 2;
    };
    let distance = forward.get(n, m);
    a.reverse();
    b.reverse();
    let backward = Matrix::build(&a, &b, band);
    a.reverse();
    b.reverse();
    let position = |index: usize| {
        let mut matched = None;
        for column in index.saturating_sub(band)..=m.min(index + band) {
            let cost = forward.get(index, column);
            if cost + 1 + backward.get(n - index - 1, m - column) == distance {
                return None;
            }
            if b.get(column) == Some(&a[index])
                && cost + backward.get(n - index - 1, m - column - 1) == distance
            {
                if matched.is_some() {
                    return None;
                }
                matched = Some(column);
            }
        }
        matched
    };
    let mapped = if local.is_empty() {
        if local.start > 0 {
            position(local.start - 1)?;
        }
        if local.start < n {
            position(local.start)?;
        }
        let mut boundary = None;
        for column in local.start.saturating_sub(band)..=m.min(local.start + band) {
            if forward.get(local.start, column) + backward.get(n - local.start, m - column)
                == distance
            {
                if boundary.is_some() {
                    return None;
                }
                boundary = Some(column);
            }
        }
        let at = boundary?;
        at..at
    } else {
        let start = position(local.start)?;
        for index in local.start + 1..local.end {
            if position(index)? != start + index - local.start {
                return None;
            }
        }
        start..start + local.len()
    };
    let start = b[..mapped.start]
        .iter()
        .map(|character| character.len_utf16())
        .sum::<usize>();
    let end = start
        + b[mapped]
            .iter()
            .map(|character| character.len_utf16())
            .sum::<usize>();
    Some(u32::try_from(start).ok()?..u32::try_from(end).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn optimal_paths(a: &[char], b: &[char]) -> Vec<Vec<(usize, usize)>> {
        fn visit(
            a: &[char],
            b: &[char],
            path: &mut Vec<(usize, usize)>,
            cost: usize,
            best: &mut usize,
            found: &mut Vec<Vec<(usize, usize)>>,
        ) {
            if cost > *best {
                return;
            }
            let (i, j) = *path.last().unwrap();
            if (i, j) == (a.len(), b.len()) {
                if cost < *best {
                    found.clear();
                    *best = cost;
                }
                found.push(path.clone());
                return;
            }
            for (next_i, next_j, charge) in [(i + 1, j + 1, 0), (i + 1, j, 1), (i, j + 1, 1)] {
                if next_i > a.len() || next_j > b.len() || (charge == 0 && a[i] != b[j]) {
                    continue;
                }
                path.push((next_i, next_j));
                visit(a, b, path, cost + charge, best, found);
                path.pop();
            }
        }
        let mut found = Vec::new();
        let mut best = usize::MAX;
        visit(a, b, &mut vec![(0, 0)], 0, &mut best, &mut found);
        found
    }

    #[test]
    fn bounded_alignment_agrees_with_exhaustive_paths_for_every_small_unicode_edit() {
        let mut words = vec![String::new()];
        for length in 1..=4 {
            for bits in 0..1 << length {
                words.push(
                    (0..length)
                        .map(|bit| if bits & (1 << bit) == 0 { 'a' } else { '🦀' })
                        .collect(),
                );
            }
        }
        let mut cases = 0;
        for before in &words {
            let a: Vec<_> = before.chars().collect();
            for after in &words {
                let b: Vec<_> = after.chars().collect();
                let paths = optimal_paths(&a, &b);
                for start in 0..=a.len() {
                    for end in start..=a.len() {
                        let mut expected = None;
                        let mut valid = true;
                        for path in &paths {
                            let mapping: Vec<_> = (0..a.len())
                                .map(|i| {
                                    path.windows(2).find_map(|edge| {
                                        (edge[0].0 == i && edge[1] == (i + 1, edge[0].1 + 1))
                                            .then_some(edge[0].1)
                                    })
                                })
                                .collect();
                            let candidate = if start == end {
                                let vertices: Vec<_> = path
                                    .iter()
                                    .filter(|(i, _)| *i == start)
                                    .map(|(_, j)| *j)
                                    .collect();
                                if (start > 0 && mapping[start - 1].is_none())
                                    || (start < a.len() && mapping[start].is_none())
                                    || vertices.len() != 1
                                {
                                    None
                                } else {
                                    Some(vertices[0]..vertices[0])
                                }
                            } else if let Some(first) = mapping[start] {
                                (start..end)
                                    .all(|i| mapping[i] == Some(first + i - start))
                                    .then_some(first..first + end - start)
                            } else {
                                None
                            };
                            let Some(candidate) = candidate else {
                                valid = false;
                                break;
                            };
                            if expected.as_ref().is_some_and(|old| *old != candidate) {
                                valid = false;
                                break;
                            }
                            expected = Some(candidate);
                        }
                        let expected = if valid {
                            expected.map(|range| {
                                b[..range.start].iter().map(|c| c.len_utf16() as u32).sum()
                                    ..b[..range.end].iter().map(|c| c.len_utf16() as u32).sum()
                            })
                        } else {
                            None
                        };
                        let range = a[..start].iter().map(|c| c.len_utf16() as u32).sum()
                            ..a[..end].iter().map(|c| c.len_utf16() as u32).sum();
                        assert_eq!(
                            rebase(before, after, range),
                            expected,
                            "{before:?} -> {after:?}, characters {start}..{end}"
                        );
                        cases += 1;
                    }
                }
            }
        }
        assert_eq!(cases, 10881);
    }

    #[test]
    fn maps_disjoint_unicode_edits_and_rejects_ambiguous_or_overlapping_changes() {
        assert_eq!(rebase("ab🦀cd", "Xab🦀cYd", 2..4), Some(3..5));
        assert_eq!(rebase("abc", "XabcY", 1..2), Some(2..3));
        assert_eq!(rebase("abc", "XabcY", 1..1), Some(2..2));
        assert_eq!(rebase("abc", "abcX", 3..3), None);
        assert_eq!(rebase("abc", "ac", 1..2), None);
        assert_eq!(rebase("abc", "", 1..1), None);
        assert_eq!(rebase("aaaa", "aaaaa", 1..2), None);
        assert_eq!(rebase("ab🦀cd", "ab🦀cd", 3..4), None);
        assert_eq!(rebase("abc", "abc", 4..4), None);
    }

    #[test]
    fn long_paragraphs_with_small_remote_changes_use_a_narrow_band() {
        let text = format!("{}🦀{}", "a".repeat(8192), "b".repeat(8192));
        assert_eq!(
            rebase(&text, &format!("X{text}Y"), 8192..8194),
            Some(8193..8195)
        );
        assert_eq!(rebase(&"a".repeat(8192), &"b".repeat(8192), 1..2), None);
    }
}
