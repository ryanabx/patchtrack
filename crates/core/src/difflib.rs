//! Python's difflib, as far as patchtrack uses it: `SequenceMatcher` (with
//! no junk function and autojunk on, as `SequenceMatcher(None, a, b)`) and
//! `unified_diff`. Ported line for line, so a diff of two versions reads
//! the same as the Python version's, down to which lines pair up.

use std::collections::HashMap;
use std::hash::Hash;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    Equal,
    Replace,
    Delete,
    Insert,
}

/// (tag, i1, i2, j1, j2): a[i1..i2] becomes b[j1..j2].
pub type Opcode = (Tag, usize, usize, usize, usize);

pub struct SequenceMatcher<'a, T> {
    a: &'a [T],
    b: &'a [T],
    /// Where each item of b is; items common in a long b are left out (autojunk).
    b2j: HashMap<&'a T, Vec<usize>>,
}

impl<'a, T: Eq + Hash> SequenceMatcher<'a, T> {
    pub fn new(a: &'a [T], b: &'a [T]) -> Self {
        let mut b2j: HashMap<&T, Vec<usize>> = HashMap::new();
        for (j, x) in b.iter().enumerate() {
            b2j.entry(x).or_default().push(j);
        }
        if b.len() >= 200 {
            let ntest = b.len() / 100 + 1;
            b2j.retain(|_, js| js.len() <= ntest);
        }
        SequenceMatcher { a, b, b2j }
    }

    /// The longest matching block in a[alo..ahi] and b[blo..bhi], earliest
    /// in a, then in b: (i, j, size).
    fn find_longest_match(&self, alo: usize, ahi: usize, blo: usize, bhi: usize) -> (usize, usize, usize) {
        let (a, b) = (self.a, self.b);
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for (i, x) in a.iter().enumerate().take(ahi).skip(alo) {
            let mut next: HashMap<usize, usize> = HashMap::new();
            for &j in self.b2j.get(x).map(Vec::as_slice).unwrap_or_default() {
                if j < blo {
                    continue;
                }
                if j >= bhi {
                    break;
                }
                let k = j.checked_sub(1).and_then(|p| j2len.get(&p)).copied().unwrap_or(0) + 1;
                next.insert(j, k);
                if k > bestsize {
                    (besti, bestj, bestsize) = (i + 1 - k, j + 1 - k, k);
                }
            }
            j2len = next;
        }
        // Items autojunk left out of b2j still extend a match at either end.
        while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
            (besti, bestj, bestsize) = (besti - 1, bestj - 1, bestsize + 1);
        }
        while besti + bestsize < ahi && bestj + bestsize < bhi && a[besti + bestsize] == b[bestj + bestsize] {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    }

    /// (i, j, n): a[i..i+n] == b[j..j+n], in order, adjacent blocks merged,
    /// ending with (len a, len b, 0).
    pub fn matching_blocks(&self) -> Vec<(usize, usize, usize)> {
        let (la, lb) = (self.a.len(), self.b.len());
        let mut queue = vec![(0, la, 0, lb)];
        let mut blocks = vec![];
        while let Some((alo, ahi, blo, bhi)) = queue.pop() {
            let (i, j, k) = self.find_longest_match(alo, ahi, blo, bhi);
            if k > 0 {
                blocks.push((i, j, k));
                if alo < i && blo < j {
                    queue.push((alo, i, blo, j));
                }
                if i + k < ahi && j + k < bhi {
                    queue.push((i + k, ahi, j + k, bhi));
                }
            }
        }
        blocks.sort_unstable();
        let mut merged: Vec<(usize, usize, usize)> = vec![];
        let (mut i1, mut j1, mut k1) = (0, 0, 0);
        for (i2, j2, k2) in blocks {
            if i1 + k1 == i2 && j1 + k1 == j2 {
                k1 += k2;
            } else {
                if k1 > 0 {
                    merged.push((i1, j1, k1));
                }
                (i1, j1, k1) = (i2, j2, k2);
            }
        }
        if k1 > 0 {
            merged.push((i1, j1, k1));
        }
        merged.push((la, lb, 0));
        merged
    }

    pub fn opcodes(&self) -> Vec<Opcode> {
        let (mut i, mut j) = (0, 0);
        let mut out = vec![];
        for (ai, bj, size) in self.matching_blocks() {
            let tag = match (i < ai, j < bj) {
                (true, true) => Some(Tag::Replace),
                (true, false) => Some(Tag::Delete),
                (false, true) => Some(Tag::Insert),
                (false, false) => None,
            };
            if let Some(tag) = tag {
                out.push((tag, i, ai, j, bj));
            }
            (i, j) = (ai + size, bj + size);
            if size > 0 {
                out.push((Tag::Equal, ai, i, bj, j));
            }
        }
        out
    }

    /// The changes, in groups with up to n lines of context each.
    pub fn grouped_opcodes(&self, n: usize) -> Vec<Vec<Opcode>> {
        let mut codes = self.opcodes();
        if codes.is_empty() {
            codes.push((Tag::Equal, 0, 1, 0, 1));
        }
        if let Some(c) = codes.first_mut()
            && c.0 == Tag::Equal
        {
            let (tag, i1, i2, j1, j2) = *c;
            *c = (tag, i1.max(i2.saturating_sub(n)), i2, j1.max(j2.saturating_sub(n)), j2);
        }
        if let Some(c) = codes.last_mut()
            && c.0 == Tag::Equal
        {
            let (tag, i1, i2, j1, j2) = *c;
            *c = (tag, i1, i2.min(i1 + n), j1, j2.min(j1 + n));
        }
        let mut groups = vec![];
        let mut group = vec![];
        for (tag, mut i1, i2, mut j1, j2) in codes {
            if tag == Tag::Equal && i2 - i1 > 2 * n {
                group.push((tag, i1, i2.min(i1 + n), j1, j2.min(j1 + n)));
                groups.push(std::mem::take(&mut group));
                (i1, j1) = (i1.max(i2.saturating_sub(n)), j1.max(j2.saturating_sub(n)));
            }
            group.push((tag, i1, i2, j1, j2));
        }
        if !group.is_empty() && !(group.len() == 1 && group[0].0 == Tag::Equal) {
            groups.push(group);
        }
        groups
    }

    /// 2 × matches / total length; 1.0 for two empty sequences.
    pub fn ratio(&self) -> f64 {
        let total = self.a.len() + self.b.len();
        if total == 0 {
            return 1.0;
        }
        let matched: usize = self.matching_blocks().iter().map(|&(_, _, k)| k).sum();
        2.0 * matched as f64 / total as f64
    }
}

/// `difflib.unified_diff(a, b, fromfile, tofile, n=n, lineterm="")`.
pub fn unified_diff(a: &[&str], b: &[&str], fromfile: &str, tofile: &str, n: usize) -> Vec<String> {
    fn range(start: usize, stop: usize) -> String {
        let length = stop - start;
        match length {
            1 => format!("{}", start + 1),
            0 => format!("{start},0"),
            _ => format!("{},{length}", start + 1),
        }
    }
    let mut out = vec![];
    for group in SequenceMatcher::new(a, b).grouped_opcodes(n) {
        if out.is_empty() {
            out.push(format!("--- {fromfile}"));
            out.push(format!("+++ {tofile}"));
        }
        let (first, last) = (group[0], group[group.len() - 1]);
        out.push(format!("@@ -{} +{} @@", range(first.1, last.2), range(first.3, last.4)));
        for (tag, i1, i2, j1, j2) in group {
            if tag == Tag::Equal {
                out.extend(a[i1..i2].iter().map(|l| format!(" {l}")));
                continue;
            }
            if matches!(tag, Tag::Replace | Tag::Delete) {
                out.extend(a[i1..i2].iter().map(|l| format!("-{l}")));
            }
            if matches!(tag, Tag::Replace | Tag::Insert) {
                out.extend(b[j1..j2].iter().map(|l| format!("+{l}")));
            }
        }
    }
    out
}
