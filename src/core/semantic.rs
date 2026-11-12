//! Semantic similarity between memos.
//!
//! Every memo gets a local TF-IDF vector (Chinese character bigrams plus Latin
//! words), so related notes, semantic search, random walk and the cognitive map
//! work offline. When an embedding model is configured, its dense vectors take
//! precedence for memos that have one.

use std::collections::{HashMap, HashSet};

use super::model::MemoId;
use super::parse;

pub type SparseVec = Vec<(u32, f32)>;

const STOP_BIGRAMS: &[&str] = &[
    "我们", "一个", "这个", "没有", "自己", "什么", "可以", "就是", "因为", "所以", "但是", "如果",
    "还是", "已经", "这样", "那么", "时候", "不是", "他们", "你们", "然后", "这些", "那些", "觉得",
    "一些", "其实", "而且", "或者", "以及", "的是", "了一", "是一", "不会", "也是", "都是", "这种",
];

const STOP_WORDS: &[&str] = &[
    "the", "and", "for", "are", "but", "not", "you", "all", "any", "can", "had", "her", "was", "one",
    "our", "out", "has", "have", "this", "that", "with", "from", "they", "will", "would", "there",
    "their", "what", "about", "which", "when", "your", "into", "than", "then", "them", "these",
    "is", "it", "of", "to", "in", "on", "a", "an", "be", "as", "at", "by", "or", "we", "do", "if",
];

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x20000..=0x2A6DF | 0xF900..=0xFAFF | 0x3040..=0x30FF | 0xAC00..=0xD7AF)
}

pub fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for tag in parse::extract_tags(text) {
        tokens.push(format!("#{tag}"));
    }
    let plain = parse::plain_text(text);
    let mut cjk_run: Vec<char> = Vec::new();
    let mut word = String::new();

    let flush_cjk = |run: &mut Vec<char>, tokens: &mut Vec<String>| {
        if run.len() == 1 {
            tokens.push(run[0].to_string());
        } else {
            for pair in run.windows(2) {
                let bigram: String = pair.iter().collect();
                if !STOP_BIGRAMS.contains(&bigram.as_str()) {
                    tokens.push(bigram);
                }
            }
        }
        run.clear();
    };
    let flush_word = |word: &mut String, tokens: &mut Vec<String>| {
        if word.chars().count() >= 2 && !STOP_WORDS.contains(&word.as_str()) {
            tokens.push(std::mem::take(word));
        }
        word.clear();
    };

    for c in plain.chars() {
        if is_cjk(c) {
            flush_word(&mut word, &mut tokens);
            cjk_run.push(c);
        } else if c.is_alphanumeric() {
            flush_cjk(&mut cjk_run, &mut tokens);
            word.extend(c.to_lowercase());
        } else {
            flush_cjk(&mut cjk_run, &mut tokens);
            flush_word(&mut word, &mut tokens);
        }
    }
    flush_cjk(&mut cjk_run, &mut tokens);
    flush_word(&mut word, &mut tokens);
    tokens
}

fn sparse_dot(a: &SparseVec, b: &SparseVec) -> f32 {
    let (mut i, mut j, mut sum) = (0, 0, 0.0);
    while i < a.len() && j < b.len() {
        match a[i].0.cmp(&b[j].0) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                sum += a[i].1 * b[j].1;
                i += 1;
                j += 1;
            }
        }
    }
    sum
}

fn dense_dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

pub fn normalize_dense(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
}

pub struct SemanticIndex {
    ids: Vec<MemoId>,
    positions: HashMap<MemoId, usize>,
    sparse: Vec<SparseVec>,
    dense: Vec<Option<Vec<f32>>>,
    vocab: HashMap<String, u32>,
    terms: Vec<String>,
    idf: Vec<f32>,
    dense_count: usize,
}

impl SemanticIndex {
    pub fn empty() -> Self {
        Self::build(&[], HashMap::new())
    }

    /// `docs` are `(id, content)` pairs; `dense` holds normalized embeddings by memo id.
    pub fn build(docs: &[(MemoId, String)], mut dense: HashMap<MemoId, Vec<f32>>) -> Self {
        let tokenized: Vec<Vec<String>> = docs.iter().map(|(_, text)| tokenize(text)).collect();
        let mut vocab: HashMap<String, u32> = HashMap::new();
        let mut terms: Vec<String> = Vec::new();
        let mut df: Vec<u32> = Vec::new();
        for tokens in &tokenized {
            let unique: HashSet<&String> = tokens.iter().collect();
            for token in unique {
                let id = *vocab.entry(token.clone()).or_insert_with(|| {
                    terms.push(token.clone());
                    df.push(0);
                    (terms.len() - 1) as u32
                });
                df[id as usize] += 1;
            }
        }
        let n = docs.len() as f32;
        let idf: Vec<f32> = df
            .iter()
            .map(|&d| ((n + 1.0) / (d as f32 + 1.0)).ln() + 1.0)
            .collect();
        let sparse = tokenized
            .iter()
            .map(|tokens| Self::weigh(tokens, &vocab, &idf))
            .collect();
        let mut dense_vecs = Vec::with_capacity(docs.len());
        let mut dense_count = 0;
        for (id, _) in docs {
            let v = dense.remove(id);
            if v.is_some() {
                dense_count += 1;
            }
            dense_vecs.push(v);
        }
        Self {
            ids: docs.iter().map(|(id, _)| id.clone()).collect(),
            positions: docs
                .iter()
                .enumerate()
                .map(|(i, (id, _))| (id.clone(), i))
                .collect(),
            sparse,
            dense: dense_vecs,
            vocab,
            terms,
            idf,
            dense_count,
        }
    }

    fn weigh(tokens: &[String], vocab: &HashMap<String, u32>, idf: &[f32]) -> SparseVec {
        let mut counts: HashMap<u32, f32> = HashMap::new();
        for token in tokens {
            if let Some(&id) = vocab.get(token) {
                let boost = if token.starts_with('#') { 2.0 } else { 1.0 };
                *counts.entry(id).or_default() += boost;
            }
        }
        let mut vec: SparseVec = counts
            .into_iter()
            .map(|(id, c)| (id, (1.0 + c.ln()) * idf[id as usize]))
            .collect();
        vec.sort_by_key(|(id, _)| *id);
        let norm = vec.iter().map(|(_, w)| w * w).sum::<f32>().sqrt();
        if norm > 0.0 {
            vec.iter_mut().for_each(|(_, w)| *w /= norm);
        }
        vec
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn dense_count(&self) -> usize {
        self.dense_count
    }

    pub fn position(&self, id: &str) -> Option<usize> {
        self.positions.get(id).copied()
    }

    pub fn similarity_at(&self, i: usize, j: usize) -> f32 {
        if let (Some(a), Some(b)) = (&self.dense[i], &self.dense[j]) {
            return dense_dot(a, b);
        }
        sparse_dot(&self.sparse[i], &self.sparse[j])
    }

    pub fn similarity(&self, a: &str, b: &str) -> f32 {
        match (self.position(a), self.position(b)) {
            (Some(i), Some(j)) => self.similarity_at(i, j),
            _ => 0.0,
        }
    }

    /// Most similar memos to `id`, best first, skipping ids rejected by `keep`.
    pub fn related(&self, id: &str, limit: usize, keep: impl Fn(&str) -> bool) -> Vec<(MemoId, f32)> {
        let Some(i) = self.position(id) else {
            return Vec::new();
        };
        let mut scored: Vec<(usize, f32)> = (0..self.len())
            .filter(|&j| j != i && keep(&self.ids[j]))
            .map(|j| (j, self.similarity_at(i, j)))
            .filter(|(_, s)| *s > 0.02)
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        scored.truncate(limit);
        scored
            .into_iter()
            .map(|(j, s)| (self.ids[j].clone(), s))
            .collect()
    }

    pub fn query_vector(&self, text: &str) -> SparseVec {
        Self::weigh(&tokenize(text), &self.vocab, &self.idf)
    }

    /// Ranks memos against free text. `query_dense` is the embedding of the query when available.
    pub fn search(
        &self,
        text: &str,
        query_dense: Option<&[f32]>,
        limit: usize,
        keep: impl Fn(&str) -> bool,
    ) -> Vec<(MemoId, f32)> {
        let q = self.query_vector(text);
        let mut scored: Vec<(usize, f32)> = (0..self.len())
            .filter(|&j| keep(&self.ids[j]))
            .map(|j| {
                let lexical = sparse_dot(&q, &self.sparse[j]);
                let score = match (query_dense, &self.dense[j]) {
                    (Some(qd), Some(d)) => 0.75 * dense_dot(qd, d) + 0.25 * lexical,
                    _ => lexical,
                };
                (j, score)
            })
            .filter(|(_, s)| *s > 0.01)
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        scored.truncate(limit);
        scored
            .into_iter()
            .map(|(j, s)| (self.ids[j].clone(), s))
            .collect()
    }

    /// The highest-weighted terms across a group of memos, used to name a cluster.
    pub fn top_terms(&self, members: &[usize], count: usize) -> Vec<String> {
        let mut weights: HashMap<u32, f32> = HashMap::new();
        for &m in members {
            for &(term, w) in &self.sparse[m] {
                *weights.entry(term).or_default() += w;
            }
        }
        let mut sorted: Vec<(u32, f32)> = weights.into_iter().collect();
        sorted.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut picked: Vec<String> = Vec::new();
        for (term, _) in sorted {
            let t = self.terms[term as usize].trim_start_matches('#').to_string();
            if t.chars().count() < 2 {
                continue;
            }
            if picked.iter().any(|p| p.contains(&t) || t.contains(p.as_str())) {
                continue;
            }
            picked.push(t);
            if picked.len() >= count {
                break;
            }
        }
        picked
    }

    /// Suggested tags for new text based on the tags of similar memos.
    pub fn suggest_tags(
        &self,
        text: &str,
        memo_tags: &HashMap<MemoId, Vec<String>>,
        limit: usize,
    ) -> Vec<String> {
        let existing = parse::extract_tags(text);
        let mut scores: HashMap<String, f32> = HashMap::new();
        for (id, score) in self.search(text, None, 30, |_| true) {
            if let Some(tags) = memo_tags.get(&id) {
                for tag in tags {
                    *scores.entry(tag.clone()).or_default() += score;
                }
            }
        }
        let mut sorted: Vec<(String, f32)> = scores
            .into_iter()
            .filter(|(t, _)| !existing.contains(t))
            .collect();
        sorted.sort_by(|a, b| b.1.total_cmp(&a.1));
        sorted.into_iter().take(limit).map(|(t, _)| t).collect()
    }
}

// ------------------------------------------------------------------ map

#[derive(Clone, Debug)]
pub struct MapPoint {
    pub id: MemoId,
    pub x: f32,
    pub y: f32,
    pub created_at: i64,
    pub peak: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct Peak {
    pub x: f32,
    pub y: f32,
    pub label: String,
    pub members: Vec<MemoId>,
}

#[derive(Clone, Debug, Default)]
pub struct MapLayout {
    pub points: Vec<MapPoint>,
    pub peaks: Vec<Peak>,
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn unit(&mut self) -> f32 {
        (self.next() % 1_000_000) as f32 / 1_000_000.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

pub const MAP_LIMIT: usize = 1500;

/// Lays memos out on a 2D plane where similar memos cluster into "mountains".
/// `created` maps memo ids to their creation time.
pub fn compute_map(index: &SemanticIndex, created: &HashMap<MemoId, i64>) -> MapLayout {
    let n = index.len().min(MAP_LIMIT);
    if n < 3 {
        let points = (0..n)
            .map(|i| MapPoint {
                id: index.ids[i].clone(),
                x: 0.3 + 0.2 * i as f32,
                y: 0.5,
                created_at: *created.get(&index.ids[i]).unwrap_or(&0),
                peak: None,
            })
            .collect();
        return MapLayout {
            points,
            peaks: Vec::new(),
        };
    }

    let k = 8.min(n - 1);
    let mut edges: Vec<(usize, usize, f32)> = Vec::new();
    for i in 0..n {
        let mut sims: Vec<(usize, f32)> = (0..n)
            .filter(|&j| j != i)
            .map(|j| (j, index.similarity_at(i, j)))
            .collect();
        sims.sort_by(|a, b| b.1.total_cmp(&a.1));
        for &(j, s) in sims.iter().take(k) {
            if s > 0.03 {
                edges.push((i, j, s));
            }
        }
    }

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut pos: Vec<(f32, f32)> = (0..n).map(|_| (rng.unit(), rng.unit())).collect();
    let epochs = 260;
    for epoch in 0..epochs {
        let lr = 1.0 - epoch as f32 / epochs as f32;
        for &(i, j, w) in &edges {
            let (dx, dy) = (pos[j].0 - pos[i].0, pos[j].1 - pos[i].1);
            let step = 0.08 * lr * w.min(1.0);
            pos[i].0 += dx * step;
            pos[i].1 += dy * step;
            pos[j].0 -= dx * step;
            pos[j].1 -= dy * step;
            for _ in 0..2 {
                let other = rng.below(n);
                if other == i {
                    continue;
                }
                let (rx, ry) = (pos[i].0 - pos[other].0, pos[i].1 - pos[other].1);
                let d2 = rx * rx + ry * ry + 0.002;
                let push = (0.0006 * lr / d2).min(0.02);
                pos[i].0 += rx * push;
                pos[i].1 += ry * push;
            }
        }
        // memos with no neighbours drift slowly towards the rim
        if epoch % 20 == 0 {
            for p in pos.iter_mut() {
                p.0 = p.0.clamp(-5.0, 5.0);
                p.1 = p.1.clamp(-5.0, 5.0);
            }
        }
    }

    let (min_x, max_x) = pos.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
    let (min_y, max_y) = pos.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
    let span_x = (max_x - min_x).max(1e-4);
    let span_y = (max_y - min_y).max(1e-4);
    let mut points: Vec<MapPoint> = (0..n)
        .map(|i| MapPoint {
            id: index.ids[i].clone(),
            // Margin keeps the outermost density kernels (about 3 bandwidths) inside the grid.
            x: 0.13 + 0.74 * (pos[i].0 - min_x) / span_x,
            y: 0.14 + 0.72 * (pos[i].1 - min_y) / span_y,
            created_at: *created.get(&index.ids[i]).unwrap_or(&0),
            peak: None,
        })
        .collect();

    let coords: Vec<(f32, f32)> = points.iter().map(|p| (p.x, p.y)).collect();
    let (gw, gh) = (64, 40);
    let grid = density_grid(&coords, gw, gh, 0.045);
    let max = grid.iter().cloned().fold(0.0f32, f32::max);
    let mut candidates: Vec<(usize, usize, f32)> = Vec::new();
    for gy in 1..gh - 1 {
        for gx in 1..gw - 1 {
            let v = grid[gy * gw + gx];
            if v < max * 0.22 {
                continue;
            }
            let is_max = (-1i32..=1).all(|dy| {
                (-1i32..=1).all(|dx| {
                    (dx == 0 && dy == 0)
                        || grid[(gy as i32 + dy) as usize * gw + (gx as i32 + dx) as usize] <= v
                })
            });
            if is_max {
                candidates.push((gx, gy, v));
            }
        }
    }
    candidates.sort_by(|a, b| b.2.total_cmp(&a.2));
    let mut peaks: Vec<Peak> = Vec::new();
    for (gx, gy, _) in candidates {
        let x = (gx as f32 + 0.5) / gw as f32;
        let y = (gy as f32 + 0.5) / gh as f32;
        if peaks
            .iter()
            .any(|p| ((p.x - x).powi(2) + (p.y - y).powi(2)).sqrt() < 0.1)
        {
            continue;
        }
        peaks.push(Peak {
            x,
            y,
            label: String::new(),
            members: Vec::new(),
        });
        if peaks.len() >= 12 {
            break;
        }
    }

    let mut member_idx: Vec<Vec<usize>> = vec![Vec::new(); peaks.len()];
    for (i, point) in points.iter_mut().enumerate() {
        let nearest = peaks
            .iter()
            .enumerate()
            .map(|(pi, p)| (pi, (p.x - point.x).powi(2) + (p.y - point.y).powi(2)))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((pi, d2)) = nearest
            && d2.sqrt() < 0.12
        {
            point.peak = Some(pi);
            member_idx[pi].push(i);
        }
    }
    for (pi, peak) in peaks.iter_mut().enumerate() {
        peak.label = index.top_terms(&member_idx[pi], 2).join(" · ");
        if peak.label.is_empty() {
            peak.label = t!("山峰 {}", pi + 1);
        }
        peak.members = member_idx[pi].iter().map(|&i| index.ids[i].clone()).collect();
    }
    peaks.retain(|p| p.members.len() >= 2);
    let remap: HashMap<String, usize> = peaks
        .iter()
        .enumerate()
        .flat_map(|(i, p)| p.members.iter().map(move |m| (m.clone(), i)))
        .collect();
    for point in points.iter_mut() {
        point.peak = remap.get(&point.id).copied();
    }

    MapLayout { points, peaks }
}

/// Gaussian kernel density on a `w × h` grid over the unit square.
pub fn density_grid(points: &[(f32, f32)], w: usize, h: usize, bandwidth: f32) -> Vec<f32> {
    let mut grid = vec![0.0f32; w * h];
    let radius_x = (bandwidth * 3.0 * w as f32).ceil() as i32;
    let radius_y = (bandwidth * 3.0 * h as f32).ceil() as i32;
    let inv = 1.0 / (2.0 * bandwidth * bandwidth);
    for &(px, py) in points {
        let cx = (px * w as f32) as i32;
        let cy = (py * h as f32) as i32;
        for gy in (cy - radius_y).max(0)..=(cy + radius_y).min(h as i32 - 1) {
            for gx in (cx - radius_x).max(0)..=(cx + radius_x).min(w as i32 - 1) {
                let x = (gx as f32 + 0.5) / w as f32;
                let y = (gy as f32 + 0.5) / h as f32;
                let d2 = (x - px).powi(2) + (y - py).powi(2);
                grid[gy as usize * w + gx as usize] += (-d2 * inv).exp();
            }
        }
    }
    grid
}

pub type Segment = ((f32, f32), (f32, f32));

/// Contour segments (unit-square coordinates) where the density crosses `level`.
pub fn marching_squares(grid: &[f32], w: usize, h: usize, level: f32) -> Vec<Segment> {
    let mut segments = Vec::new();
    let at = |x: usize, y: usize| grid[y * w + x];
    let point = |x: f32, y: f32| ((x + 0.5) / w as f32, (y + 0.5) / h as f32);
    let lerp = |a: f32, b: f32| {
        if (b - a).abs() < 1e-9 {
            0.5
        } else {
            ((level - a) / (b - a)).clamp(0.0, 1.0)
        }
    };
    for y in 0..h - 1 {
        for x in 0..w - 1 {
            let (tl, tr, br, bl) = (at(x, y), at(x + 1, y), at(x + 1, y + 1), at(x, y + 1));
            let case = ((tl > level) as u8) << 3
                | ((tr > level) as u8) << 2
                | ((br > level) as u8) << 1
                | (bl > level) as u8;
            if case == 0 || case == 15 {
                continue;
            }
            let (fx, fy) = (x as f32, y as f32);
            let top = point(fx + lerp(tl, tr), fy);
            let right = point(fx + 1.0, fy + lerp(tr, br));
            let bottom = point(fx + lerp(bl, br), fy + 1.0);
            let left = point(fx, fy + lerp(tl, bl));
            let mut push = |a, b| segments.push((a, b));
            match case {
                1 | 14 => push(left, bottom),
                2 | 13 => push(bottom, right),
                3 | 12 => push(left, right),
                4 | 11 => push(top, right),
                5 => {
                    push(left, top);
                    push(bottom, right);
                }
                6 | 9 => push(top, bottom),
                7 | 8 => push(left, top),
                10 => {
                    push(top, right);
                    push(left, bottom);
                }
                _ => {}
            }
        }
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn related_prefers_shared_topic() {
        let docs = vec![
            ("a".to_string(), "今天读了一本关于产品设计的书 #读书".to_string()),
            ("b".to_string(), "产品设计需要理解用户的真实需求".to_string()),
            ("c".to_string(), "晚饭吃了火锅，很开心".to_string()),
        ];
        let index = SemanticIndex::build(&docs, HashMap::new());
        let related = index.related("a", 2, |_| true);
        assert_eq!(related[0].0, "b");
        let found = index.search("用户需求", None, 3, |_| true);
        assert_eq!(found[0].0, "b");
    }

    #[test]
    fn contours_exist_around_cluster() {
        let pts = vec![(0.5, 0.5), (0.52, 0.5), (0.5, 0.52)];
        let grid = density_grid(&pts, 20, 20, 0.05);
        let segs = marching_squares(&grid, 20, 20, 0.5);
        assert!(!segs.is_empty());
    }
}
