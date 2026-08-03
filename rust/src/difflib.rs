//! Exact port of CPython difflib.SequenceMatcher(autojunk=True).ratio().
//! ratio = 2*M/T with M from the recursive longest-contiguous-match
//! decomposition; junk purging applies when len(b) >= 200.

use std::collections::{HashMap, HashSet};

fn matching_total(a: &[char], b: &[char]) -> usize {
    let mut b2j: HashMap<char, Vec<usize>> = HashMap::new();
    for (index, &ch) in b.iter().enumerate() {
        b2j.entry(ch).or_default().push(index);
    }
    if b.len() >= 200 {
        let ntest = b.len() / 100 + 1;
        let popular: HashSet<char> = b2j
            .iter()
            .filter(|(_, idx)| idx.len() > ntest)
            .map(|(&ch, _)| ch)
            .collect();
        for ch in popular {
            b2j.remove(&ch);
        }
    }
    let mut total = 0usize;
    let mut queue: Vec<(usize, usize, usize, usize)> = vec![(0, a.len(), 0, b.len())];
    while !queue.is_empty() {
        let (alo, ahi, blo, bhi) = queue.remove(0);
        let (besti, bestj, bestsize) = longest_match(a, &b2j, alo, ahi, blo, bhi);
        if bestsize > 0 {
            total += bestsize;
            if alo < besti && blo < bestj {
                queue.push((alo, besti, blo, bestj));
            }
            if besti + bestsize < ahi && bestj + bestsize < bhi {
                queue.push((besti + bestsize, ahi, bestj + bestsize, bhi));
            }
        }
    }
    total
}

fn longest_match(
    a: &[char],
    b2j: &HashMap<char, Vec<usize>>,
    alo: usize,
    ahi: usize,
    blo: usize,
    bhi: usize,
) -> (usize, usize, usize) {
    let mut besti = alo;
    let mut bestj = blo;
    let mut bestsize = 0usize;
    let mut j2len: HashMap<usize, usize> = HashMap::new();
    #[allow(clippy::needless_range_loop)]
    for i in alo..ahi {
        let mut new_j2len: HashMap<usize, usize> = HashMap::new();
        if let Some(indices) = b2j.get(&a[i]) {
            for &j in indices {
                if j < blo {
                    continue;
                }
                if j >= bhi {
                    break;
                }
                let k = j2len.get(&j.wrapping_sub(1)).copied().unwrap_or(0) + 1;
                new_j2len.insert(j, k);
                if k > bestsize {
                    besti = i + 1 - k;
                    bestj = j + 1 - k;
                    bestsize = k;
                }
            }
        }
        j2len = new_j2len;
    }
    (besti, bestj, bestsize)
}

pub fn ratio(a: &str, b: &str) -> f64 {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let length = a_chars.len() + b_chars.len();
    if length == 0 {
        return 1.0;
    }
    2.0 * matching_total(&a_chars, &b_chars) as f64 / length as f64
}
