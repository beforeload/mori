//! Graph view (关系图谱), in the spirit of Obsidian's: notes and tags are nodes, references,
//! annotations and tags are links, and a live force simulation lays them out. Scroll or pinch
//! to zoom, drag the canvas to pan, drag a node to move it, hover to light up its
//! neighbours, click to inspect, double-click to open.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AppContext as _, BorderStyle, Bounds, Context, Corners, Edges, Entity, Hsla,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement as _, PathBuilder, PinchEvent, Pixels, Point, Render, ScrollEvent, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, TextAlign, TextRun, Window,
    canvas, div, fill, point, prelude::FluentBuilder as _, px, quad, size,
};

use crate::core::graph::Simulation;
use crate::core::model::MemoId;
use crate::core::parse;

use super::library::Library;
use super::memo_card::mini_card;
use super::sidebar::sidebar_toggle;
use super::theme::surfaces;
use super::{Command, Lucide, dispatch};

const SEMANTIC_THRESHOLD: f32 = 0.18;
const MIN_SCALE: f32 = 0.05;
const MAX_SCALE: f32 = 6.0;
const DOUBLE_CLICK: Duration = Duration::from_millis(350);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum NodeKey {
    Note(MemoId),
    Tag(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinkKind {
    Tag,
    Reference,
    Semantic,
}

struct NodeMeta {
    key: NodeKey,
    label: String,
    /// What sits under the dot when the node isn't in focus (last tag segment).
    short: String,
    /// Lowercased text the search box matches against.
    haystack: String,
}

enum Drag {
    Pan { start: Point<Pixels>, offset: (f32, f32) },
    Node { index: usize },
}

pub struct GraphPage {
    sim: Option<Simulation>,
    meta: Vec<NodeMeta>,
    link_kinds: Vec<LinkKind>,
    neighbors: Vec<Vec<usize>>,
    show_tags: bool,
    show_orphans: bool,
    include_semantic: bool,
    semantic_pairs: Vec<(MemoId, MemoId)>,
    computing: bool,
    built_revision: Option<u64>,
    scale: f32,
    /// Screen offset of the world origin from the canvas centre.
    offset: (f32, f32),
    /// Keeps the whole graph framed until the user zooms or pans.
    auto_fit: bool,
    hovered: Option<usize>,
    selected: Option<usize>,
    drag: Option<Drag>,
    press: Option<Point<Pixels>>,
    drag_distance: f32,
    last_click: Option<(usize, Instant)>,
    search: Entity<InputState>,
    query: String,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    _task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl GraphPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let library = Library::global(cx);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("搜索节点…")));
        let subscriptions = vec![
            cx.observe(&library, |this, _, cx| this.on_library_changed(cx)),
            cx.subscribe_in(&search, window, |this, state, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.query = state.read(cx).value().trim().to_lowercase();
                    cx.notify();
                }
            }),
        ];
        let mut page = Self {
            sim: None,
            meta: Vec::new(),
            link_kinds: Vec::new(),
            neighbors: Vec::new(),
            show_tags: true,
            show_orphans: true,
            include_semantic: false,
            semantic_pairs: Vec::new(),
            computing: false,
            built_revision: None,
            scale: 1.0,
            offset: (0.0, 0.0),
            auto_fit: true,
            hovered: None,
            selected: None,
            drag: None,
            press: None,
            drag_distance: 0.0,
            last_click: None,
            search,
            query: String::new(),
            bounds: Rc::new(Cell::new(Bounds::default())),
            _task: None,
            _subscriptions: subscriptions,
        };
        page.rebuild(cx);
        page
    }

    fn on_library_changed(&mut self, cx: &mut Context<Self>) {
        let revision = Library::global(cx).read(cx).store().revision();
        if self.built_revision != Some(revision) {
            if self.include_semantic {
                self.compute_semantic(cx);
            } else {
                self.rebuild(cx);
            }
        }
    }

    /// Rebuilds nodes and links; nodes that survive keep their position so edits don't reshuffle.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let library = Library::global(cx);
        let lib = library.read(cx);
        let store = lib.store();
        self.built_revision = Some(store.revision());

        let mut keys: Vec<NodeKey> = Vec::new();
        let mut meta: Vec<NodeMeta> = Vec::new();
        let mut index: HashMap<NodeKey, usize> = HashMap::new();
        let mut add = |key: NodeKey, label: String, haystack: String, keys: &mut Vec<NodeKey>, meta: &mut Vec<NodeMeta>| -> usize {
            *index.entry(key.clone()).or_insert_with(|| {
                keys.push(key.clone());
                let short = match &key {
                    NodeKey::Tag(tag) => format!("#{}", tag.rsplit('/').next().unwrap_or(tag)),
                    NodeKey::Note(_) => label.clone(),
                };
                meta.push(NodeMeta { key, label, short, haystack });
                keys.len() - 1
            })
        };

        let memos: Vec<_> = store.active_memos().collect();
        for memo in &memos {
            add(NodeKey::Note(memo.id.clone()), note_label(&memo.content), memo.content.to_lowercase(), &mut keys, &mut meta);
        }
        let mut links: Vec<(usize, usize, LinkKind)> = Vec::new();
        let mut seen: HashSet<(usize, usize)> = HashSet::new();
        let mut link = |a: usize, b: usize, kind: LinkKind, links: &mut Vec<(usize, usize, LinkKind)>| {
            if a != b && seen.insert((a.min(b), a.max(b))) {
                links.push((a, b, kind));
            }
        };
        let note_index: HashMap<&str, usize> = memos.iter().enumerate().map(|(i, m)| (m.id.as_str(), i)).collect();
        let wiki = crate::core::frontmatter::LinkIndex::build(memos.iter().map(|m| (m.id.as_str(), m.content.as_str())));
        for (i, memo) in memos.iter().enumerate() {
            for r in &memo.refs {
                if let Some(&j) = note_index.get(r.as_str()) {
                    link(i, j, LinkKind::Reference, &mut links);
                }
            }
            for target in &memo.wikilinks {
                for id in wiki.resolve(target) {
                    if let Some(&j) = note_index.get(id.as_str()) {
                        link(i, j, LinkKind::Reference, &mut links);
                    }
                }
            }
        }
        for (a, b) in &self.semantic_pairs {
            if let (Some(&i), Some(&j)) = (note_index.get(a.as_str()), note_index.get(b.as_str())) {
                link(i, j, LinkKind::Semantic, &mut links);
            }
        }
        if self.show_tags {
            for tag in lib.tag_counts().keys() {
                let t = add(NodeKey::Tag(tag.clone()), format!("#{tag}"), format!("#{}", tag.to_lowercase()), &mut keys, &mut meta);
                if let Some((parent, _)) = tag.rsplit_once('/') {
                    let p = add(NodeKey::Tag(parent.to_string()), format!("#{parent}"), format!("#{}", parent.to_lowercase()), &mut keys, &mut meta);
                    link(t, p, LinkKind::Tag, &mut links);
                }
            }
            for (i, memo) in memos.iter().enumerate() {
                for tag in &memo.tags {
                    if let Some(&t) = index.get(&NodeKey::Tag(tag.clone())) {
                        link(i, t, LinkKind::Tag, &mut links);
                    }
                }
            }
        }

        // Optionally drop notes without any link, then compact indices.
        let mut degree = vec![0usize; keys.len()];
        for &(a, b, _) in &links {
            degree[a] += 1;
            degree[b] += 1;
        }
        let keep: Vec<bool> = (0..keys.len()).map(|i| self.show_orphans || degree[i] > 0).collect();
        let mut remap = vec![usize::MAX; keys.len()];
        let mut kept_meta = Vec::new();
        for (i, m) in meta.into_iter().enumerate() {
            if keep[i] {
                remap[i] = kept_meta.len();
                kept_meta.push(m);
            }
        }
        let links: Vec<(usize, usize, LinkKind)> = links
            .into_iter()
            .filter(|&(a, b, _)| keep[a] && keep[b])
            .map(|(a, b, k)| (remap[a], remap[b], k))
            .collect();

        let previous: HashMap<NodeKey, (f32, f32)> = match &self.sim {
            Some(sim) => self.meta.iter().zip(&sim.nodes).map(|(m, n)| (m.key.clone(), (n.x, n.y))).collect(),
            None => HashMap::new(),
        };
        let selected_key = self.selected.and_then(|i| self.meta.get(i)).map(|m| m.key.clone());

        let mut sim = Simulation::new(kept_meta.len(), links.iter().map(|&(a, b, _)| (a, b)).collect());
        let mut reused = 0;
        for (node, m) in sim.nodes.iter_mut().zip(&kept_meta) {
            if let Some(&(x, y)) = previous.get(&m.key) {
                node.x = x;
                node.y = y;
                reused += 1;
            }
        }
        if reused > 0 && reused * 10 >= kept_meta.len() * 9 {
            // Mostly the same graph: settle locally instead of re-running the whole layout.
            sim.set_alpha(0.25);
        }

        let mut neighbors = vec![Vec::new(); kept_meta.len()];
        for &(a, b, _) in &links {
            neighbors[a].push(b);
            neighbors[b].push(a);
        }
        self.link_kinds = links.iter().map(|&(_, _, k)| k).collect();
        self.selected = selected_key.and_then(|k| kept_meta.iter().position(|m| m.key == k));
        self.hovered = None;
        self.meta = kept_meta;
        self.neighbors = neighbors;
        self.sim = Some(sim);
        cx.notify();
    }

    fn compute_semantic(&mut self, cx: &mut Context<Self>) {
        let library = Library::global(cx);
        let lib = library.read(cx);
        let ids: Vec<MemoId> = lib.store().active_memos().map(|m| m.id.clone()).collect();
        let index = lib.index();
        self.computing = true;
        cx.notify();
        self._task = Some(cx.spawn(async move |this, cx| {
            let pairs = cx
                .background_spawn(async move {
                    let mut pairs = Vec::new();
                    for id in &ids {
                        for (other, score) in index.related(id, 2, |_| true) {
                            if score >= SEMANTIC_THRESHOLD && id < &other {
                                pairs.push((id.clone(), other));
                            }
                        }
                    }
                    pairs
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.semantic_pairs = pairs;
                this.computing = false;
                this.rebuild(cx);
            });
        }));
    }

    fn center(&self) -> Point<Pixels> {
        self.bounds.get().center()
    }

    fn to_screen(&self, x: f32, y: f32) -> Point<Pixels> {
        let c = self.center();
        point(c.x + px(self.offset.0 + x * self.scale), c.y + px(self.offset.1 + y * self.scale))
    }

    fn to_world(&self, p: Point<Pixels>) -> (f32, f32) {
        let c = self.center();
        (
            ((p.x - c.x).as_f32() - self.offset.0) / self.scale,
            ((p.y - c.y).as_f32() - self.offset.1) / self.scale,
        )
    }

    fn radius(&self, i: usize) -> f32 {
        let degree = self.sim.as_ref().map(|s| s.degree[i]).unwrap_or(0);
        let world = 3.5 + (degree as f32).sqrt() * 1.9;
        (world * self.scale.powf(0.75)).clamp(1.8, 26.0)
    }

    fn hit_test(&self, p: Point<Pixels>) -> Option<usize> {
        let sim = self.sim.as_ref()?;
        let mut best: Option<(f32, usize)> = None;
        for (i, node) in sim.nodes.iter().enumerate() {
            let s = self.to_screen(node.x, node.y);
            let d = ((s.x - p.x).as_f32().powi(2) + (s.y - p.y).as_f32().powi(2)).sqrt();
            let reach = self.radius(i) + 4.0;
            if d <= reach && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, i));
            }
        }
        best.map(|(_, i)| i)
    }

    fn zoom_at(&mut self, p: Point<Pixels>, factor: f32) {
        let (wx, wy) = self.to_world(p);
        self.scale = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        let c = self.center();
        self.offset = ((p.x - c.x).as_f32() - wx * self.scale, (p.y - c.y).as_f32() - wy * self.scale);
        self.auto_fit = false;
    }

    fn fit_target(&self) -> Option<(f32, (f32, f32))> {
        let (min_x, min_y, max_x, max_y) = self.sim.as_ref()?.extent()?;
        let b = self.bounds.get();
        let (w, h) = (b.size.width.as_f32() - 120.0, b.size.height.as_f32() - 120.0);
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        let scale = (w / (max_x - min_x).max(1.0)).min(h / (max_y - min_y).max(1.0)).clamp(MIN_SCALE, 2.0);
        let (cx, cy) = ((min_x + max_x) / 2.0, (min_y + max_y) / 2.0);
        Some((scale, (-cx * scale, -cy * scale)))
    }

    /// Moves the view toward the fitted frame; returns whether it still has ground to cover.
    fn approach_fit(&mut self) -> bool {
        let Some((scale, offset)) = self.fit_target() else {
            return false;
        };
        let t = 0.18;
        self.scale += (scale - self.scale) * t;
        self.offset.0 += (offset.0 - self.offset.0) * t;
        self.offset.1 += (offset.1 - self.offset.1) * t;
        (scale - self.scale).abs() > scale * 0.002 || (offset.0 - self.offset.0).abs() > 0.5 || (offset.1 - self.offset.1).abs() > 0.5
    }

    fn step(&mut self, window: &mut Window) {
        let mut animating = false;
        if let Some(sim) = self.sim.as_mut()
            && sim.is_running()
        {
            let ticks = if sim.nodes.len() > 1200 { 1 } else { 2 };
            for _ in 0..ticks {
                sim.tick();
            }
            animating = true;
        }
        if self.auto_fit {
            let moving = self.approach_fit();
            if !moving && !animating {
                self.auto_fit = false;
            }
            animating |= moving;
        }
        if animating {
            window.request_animation_frame();
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        self.press = Some(event.position);
        self.drag_distance = 0.0;
        if let Some(i) = self.hit_test(event.position) {
            if let Some(sim) = self.sim.as_mut() {
                let n = &mut sim.nodes[i];
                n.fixed = Some((n.x, n.y));
                sim.set_alpha_target(0.2);
            }
            self.drag = Some(Drag::Node { index: i });
        } else {
            self.drag = Some(Drag::Pan { start: event.position, offset: self.offset });
        }
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if let Some(press) = self.press {
            let d = ((event.position.x - press.x).as_f32().powi(2) + (event.position.y - press.y).as_f32().powi(2)).sqrt();
            self.drag_distance = self.drag_distance.max(d);
        }
        if self.drag.is_some() && event.pressed_button != Some(MouseButton::Left) {
            self.end_drag();
        }
        match &self.drag {
            Some(Drag::Pan { start, offset }) => {
                self.offset = (offset.0 + (event.position.x - start.x).as_f32(), offset.1 + (event.position.y - start.y).as_f32());
                if self.drag_distance > 3.0 {
                    self.auto_fit = false;
                }
                cx.notify();
            }
            Some(Drag::Node { index }) => {
                let index = *index;
                let world = self.to_world(event.position);
                if let Some(sim) = self.sim.as_mut() {
                    sim.nodes[index].fixed = Some(world);
                }
                self.hovered = Some(index);
                cx.notify();
            }
            None => {
                let hovered = self.hit_test(event.position);
                if hovered != self.hovered {
                    self.hovered = hovered;
                    cx.notify();
                }
            }
        }
    }

    fn end_drag(&mut self) {
        if let Some(Drag::Node { index }) = self.drag.take()
            && let Some(sim) = self.sim.as_mut()
        {
            sim.nodes[index].fixed = None;
            sim.set_alpha_target(0.0);
        }
        self.press = None;
    }

    fn on_mouse_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let clicked = self.drag_distance < 4.0;
        let target = match &self.drag {
            Some(Drag::Node { index }) => Some(*index),
            _ => None,
        };
        self.end_drag();
        if clicked {
            match target {
                Some(i) => self.click(i, window, cx),
                None => self.selected = None,
            }
        }
        cx.notify();
    }

    fn click(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        let double = self.last_click.is_some_and(|(j, at)| j == i && now.duration_since(at) < DOUBLE_CLICK);
        self.last_click = Some((i, now));
        self.selected = Some(i);
        if double && let Some(m) = self.meta.get(i) {
            // The detail panel takes over; the inspector would only repeat it.
            self.selected = None;
            let command = match &m.key {
                NodeKey::Note(id) => Command::OpenMemo(id.clone()),
                NodeKey::Tag(tag) => Command::OpenTag(tag.clone()),
            };
            dispatch(command, window, cx);
        }
    }

    fn render_canvas(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let s = surfaces(cx);
        if self.meta.is_empty() {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.muted_foreground)
                .child(if self.computing { t!("正在计算语义关联…") } else { t!("还没有可以连接的笔记") })
                .into_any_element();
        }
        self.step(window);
        let sim = self.sim.as_ref().expect("graph built");

        let focus = self.hovered.or(self.selected);
        let focus_set: Option<HashSet<usize>> = focus.map(|f| {
            let mut set: HashSet<usize> = self.neighbors[f].iter().copied().collect();
            set.insert(f);
            set
        });
        let matches: Option<HashSet<usize>> = (!self.query.is_empty()).then(|| {
            self.meta.iter().enumerate().filter(|(_, m)| m.haystack.contains(&self.query)).map(|(i, _)| i).collect()
        });

        let positions: Vec<Point<Pixels>> = sim.nodes.iter().map(|n| self.to_screen(n.x, n.y)).collect();
        let radii: Vec<f32> = (0..positions.len()).map(|i| self.radius(i)).collect();
        let note_color = theme.muted_foreground;
        let tag_color = s.accent;
        let highlight = s.accent;
        let colors: Vec<Hsla> = (0..positions.len())
            .map(|i| {
                let base = if matches!(self.meta[i].key, NodeKey::Tag(_)) { tag_color } else { note_color };
                let dimmed = focus_set.as_ref().is_some_and(|set| !set.contains(&i))
                    || matches.as_ref().is_some_and(|set| !set.contains(&i));
                if focus == Some(i) {
                    highlight
                } else if dimmed {
                    base.opacity(0.14)
                } else {
                    base.opacity(0.9)
                }
            })
            .collect();

        let mut plain: Vec<(usize, usize)> = Vec::new();
        let mut lit: Vec<(usize, usize)> = Vec::new();
        let mut semantic: Vec<(usize, usize)> = Vec::new();
        for (k, &(a, b)) in sim.links.iter().enumerate() {
            if focus.is_some_and(|f| a == f || b == f) {
                lit.push((a, b));
            } else if self.link_kinds[k] == LinkKind::Semantic {
                semantic.push((a, b));
            } else {
                plain.push((a, b));
            }
        }
        let quiet = focus.is_some() || matches.is_some();
        let line_width = (0.9 * self.scale.sqrt()).clamp(0.5, 1.6);
        let edge_color = theme.muted_foreground.opacity(if quiet { 0.07 } else { 0.24 });
        let semantic_color = theme.blue.opacity(if quiet { 0.06 } else { 0.22 });

        let base_alpha = ((self.scale - 0.8) / 0.4).clamp(0.0, 1.0);
        // Only the biggest hubs keep a label when zoomed out, as in Obsidian.
        let hub_cutoff = {
            let mut degrees = sim.degree.clone();
            degrees.sort_unstable_by(|a, b| b.cmp(a));
            degrees.get(18).copied().unwrap_or(0).max(4)
        };
        let hub_alpha = ((self.scale - 0.12) / 0.2).clamp(0.0, 1.0);
        let font_size = (10.5 * self.scale.powf(0.35)).clamp(9.0, 14.0);
        let bounds = self.bounds.get();
        let mut order: Vec<usize> = (0..positions.len()).collect();
        let priority = |i: usize| -> (bool, bool, usize) {
            (
                focus == Some(i) || self.selected == Some(i),
                focus_set.as_ref().is_some_and(|set| set.contains(&i)) || matches.as_ref().is_some_and(|set| set.contains(&i)),
                sim.degree[i],
            )
        };
        order.sort_by_key(|&i| std::cmp::Reverse(priority(i)));
        // Labels are placed by priority and skipped where they would overlap one already placed.
        let mut placed: Vec<Bounds<Pixels>> = Vec::new();
        let labels: Vec<(Point<Pixels>, String, Hsla, f32)> = order
            .into_iter()
            .filter_map(|i| {
                let p = positions[i];
                let margin = px(80.);
                if p.x < bounds.left() - margin || p.x > bounds.right() + margin || p.y < bounds.top() - margin || p.y > bounds.bottom() + margin {
                    return None;
                }
                let degree = sim.degree[i];
                let mut alpha = if degree >= hub_cutoff { hub_alpha.max(base_alpha) } else { base_alpha };
                if let Some(set) = &focus_set {
                    alpha = if set.contains(&i) { 1.0 } else { alpha * 0.25 };
                }
                if let Some(set) = &matches {
                    alpha = if set.contains(&i) { 1.0 } else { alpha * 0.25 };
                }
                if self.selected == Some(i) {
                    alpha = 1.0;
                }
                if alpha <= 0.03 {
                    return None;
                }
                let text = if focus == Some(i) || self.selected == Some(i) { &self.meta[i].label } else { &self.meta[i].short };
                let width = estimate_width(text, font_size);
                let at = point(p.x, p.y + px(radii[i] + 3.0));
                let rect = Bounds::new(point(at.x - px(width / 2.0 + 2.0), at.y), size(px(width + 4.0), px(font_size * 1.3)));
                if placed.iter().any(|r| r.intersects(&rect)) {
                    return None;
                }
                placed.push(rect);
                let color = if focus == Some(i) { theme.foreground } else { theme.foreground.opacity(0.62) };
                Some((at, text.clone(), color.opacity(color.a * alpha), alpha))
            })
            .collect();
        let ring_color = theme.background;
        let selected = self.selected;
        let bg = s.page;
        let bounds_cell = self.bounds.clone();

        div()
            .id("graph-canvas")
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(bg)
            .when(self.hovered.is_some(), |d| d.cursor_pointer())
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| this.on_mouse_down(e, cx)))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| this.on_mouse_move(e, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, window, cx| this.on_mouse_up(window, cx)))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered && this.drag.is_none() && this.hovered.is_some() {
                    this.hovered = None;
                    cx.notify();
                }
            }))
            .on_scroll(cx.listener(|this, event: &ScrollEvent, window, cx| {
                // Like Obsidian: scrolling zooms around the pointer, dragging pans.
                let delta = event.delta.pixel_delta(window.line_height());
                if delta.y.as_f32().abs() >= delta.x.as_f32().abs() {
                    this.zoom_at(event.position, (delta.y.as_f32() * 0.003).exp());
                } else {
                    this.offset.0 += delta.x.as_f32();
                    this.auto_fit = false;
                }
                cx.notify();
            }))
            .on_pinch(cx.listener(|this, event: &PinchEvent, _, cx| {
                this.zoom_at(event.position, 1.0 + event.delta);
                cx.notify();
            }))
            .child(
                canvas(
                    move |b, _, _| bounds_cell.set(b),
                    move |_, _, window, cx| {
                        for (segments, color, width) in [
                            (&semantic, semantic_color, line_width),
                            (&plain, edge_color, line_width),
                            (&lit, highlight.opacity(0.85), line_width + 0.6),
                        ] {
                            if segments.is_empty() {
                                continue;
                            }
                            let mut builder = PathBuilder::stroke(px(width));
                            for &(a, b) in segments.iter() {
                                builder.move_to(positions[a]);
                                builder.line_to(positions[b]);
                            }
                            if let Ok(path) = builder.build() {
                                window.paint_path(path, color);
                            }
                        }
                        for (i, p) in positions.iter().enumerate() {
                            let r = px(radii[i]);
                            let ring = if selected == Some(i) { px(2.) } else { px(0.) };
                            window.paint_quad(quad(
                                Bounds::new(point(p.x - r, p.y - r), size(r * 2., r * 2.)),
                                Corners::all(r),
                                colors[i],
                                Edges::all(ring),
                                ring_color,
                                BorderStyle::default(),
                            ));
                        }
                        let font = window.text_style().font();
                        for (at, text, color, alpha) in &labels {
                            let run = TextRun {
                                len: text.len(),
                                font: font.clone(),
                                color: *color,
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            };
                            let shaped = window.text_system().shape_line(SharedString::from(text.clone()), px(font_size), &[run], None);
                            let width = shaped.width();
                            let line_height = px(font_size * 1.3);
                            window.paint_quad(fill(
                                Bounds::new(point(at.x - width / 2. - px(3.), at.y), size(width + px(6.), line_height)),
                                bg.opacity(0.78 * alpha),
                            ).corner_radii(px(3.)));
                            let _ = shaped.paint(point(at.x - width / 2., at.y), line_height, TextAlign::Left, None, window, cx);
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
            .child(self.render_controls(cx))
            .when_some(self.render_inspector(cx), |this, panel| this.child(panel))
            .into_any_element()
    }

    fn render_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let s = surfaces(cx);
        let zoom = format!("{:.0}%", self.scale * 100.0);
        let tool = |id: &'static str, icon: Lucide, tip: &'static str| Button::new(id).ghost().small().icon(Icon::new(icon)).tooltip(tip);
        h_flex()
            .absolute()
            .left_3()
            .bottom_3()
            .gap_0p5()
            .p_1()
            .rounded_lg()
            .bg(s.card.opacity(0.94))
            .border_1()
            .border_color(theme.border)
            .shadow_md()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(tool("graph-zoom-out", Lucide::Minus, t!("缩小")).on_click(cx.listener(|this, _, _, cx| {
                this.zoom_at(this.center(), 1.0 / 1.25);
                cx.notify();
            })))
            .child(div().w(px(44.)).text_center().text_xs().text_color(theme.muted_foreground).child(zoom))
            .child(tool("graph-zoom-in", Lucide::Plus, t!("放大")).on_click(cx.listener(|this, _, _, cx| {
                this.zoom_at(this.center(), 1.25);
                cx.notify();
            })))
            .child(div().w_px().h_4().bg(theme.border).mx_1())
            .child(tool("graph-fit", Lucide::Scan, t!("适应窗口")).on_click(cx.listener(|this, _, _, cx| {
                this.auto_fit = true;
                cx.notify();
            })))
            .child(tool("graph-relayout", Lucide::Shuffle, t!("重新布局")).on_click(cx.listener(|this, _, _, cx| {
                this.sim = None;
                this.auto_fit = true;
                this.rebuild(cx);
            })))
            .into_any_element()
    }

    fn render_inspector(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let i = self.selected?;
        let meta = self.meta.get(i)?;
        let library = Library::global(cx);
        let lib = library.read(cx);
        let theme = cx.theme().clone();
        let s = surfaces(cx);
        let mut neighbor_notes = Vec::new();
        let mut neighbor_tags = Vec::new();
        for &j in &self.neighbors[i] {
            match &self.meta[j].key {
                NodeKey::Note(id) => {
                    if let Some(m) = lib.store().get_active(id) {
                        neighbor_notes.push(m.clone());
                    }
                }
                NodeKey::Tag(tag) => neighbor_tags.push(tag.clone()),
            }
        }
        neighbor_notes.sort_by_key(|m| std::cmp::Reverse(m.created_at));
        neighbor_tags.sort();
        let total_notes = neighbor_notes.len();
        neighbor_notes.truncate(40);

        let head = match &meta.key {
            NodeKey::Note(id) => {
                let memo = lib.store().get_active(id)?.clone();
                v_flex()
                    .gap_2()
                    .child(div().text_xs().text_color(theme.muted_foreground).child(t!("笔记")))
                    .child(mini_card("graph-selected", &memo, None, cx))
                    .into_any_element()
            }
            NodeKey::Tag(tag) => {
                let open = tag.clone();
                v_flex()
                    .gap_1()
                    .child(div().text_xs().text_color(theme.muted_foreground).child(t!("标签")))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().text_base().font_semibold().text_color(s.accent).child(format!("#{tag}")))
                            .child(div().flex_1())
                            .child(Button::new("graph-open-tag").ghost().xsmall().label(t!("查看全部")).on_click(move |_, window, cx| {
                                dispatch(Command::OpenTag(open.clone()), window, cx)
                            })),
                    )
                    .into_any_element()
            }
        };
        // Around a tag, show its family by the part that differs: `#工作` above, `/AIGC` below.
        let selected_tag = match &meta.key {
            NodeKey::Tag(tag) => Some(tag.clone()),
            NodeKey::Note(_) => None,
        };
        let tag_chips = h_flex().flex_wrap().gap_1().children(neighbor_tags.into_iter().map(|tag| {
            let open = tag.clone();
            let text = match &selected_tag {
                Some(parent) if tag.starts_with(&format!("{parent}/")) => format!("/{}", &tag[parent.len() + 1..]),
                _ => format!("#{tag}"),
            };
            div()
                .id(SharedString::from(format!("graph-tag-{tag}")))
                .px_1p5()
                .py_0p5()
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .text_color(s.accent)
                .bg(s.accent.opacity(0.1))
                .hover(|d| d.bg(s.accent.opacity(0.18)))
                .child(text)
                .on_click(move |_, window, cx| dispatch(Command::OpenTag(open.clone()), window, cx))
        }));
        let body = v_flex()
                .id("graph-inspector")
                .size_full()
                .p_3()
                .gap_3()
                .overflow_y_scrollbar()
                .child(
                    h_flex()
                        .child(div().flex_1().child(head))
                        .child(Button::new("graph-inspector-close").ghost().xsmall().icon(Icon::new(Lucide::X)).on_click(cx.listener(|this, _, _, cx| {
                            this.selected = None;
                            cx.notify();
                        }))),
                )
                .when(!tag_chips_is_empty(&self.neighbors[i], &self.meta), |this| this.child(tag_chips))
                .child(div().text_xs().text_color(theme.muted_foreground).child(t!("相连的笔记 {}", total_notes)))
                .children(neighbor_notes.iter().map(|m| mini_card("graph-neighbor", m, None, cx)));
        // `overflow_y_scrollbar` wraps its element, so position an outer box instead.
        Some(
            div()
                .id("graph-inspector-frame")
                .absolute()
                .top_3()
                .right_3()
                .bottom(px(64.))
                .w(px(300.))
                .rounded_lg()
                .overflow_hidden()
                .bg(s.card.opacity(0.97))
                .border_1()
                .border_color(theme.border)
                .shadow_lg()
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_scroll(|_, _, cx| cx.stop_propagation())
                .child(body)
                .into_any_element(),
        )
    }
}

fn tag_chips_is_empty(neighbors: &[usize], meta: &[NodeMeta]) -> bool {
    !neighbors.iter().any(|&j| matches!(meta[j].key, NodeKey::Tag(_)))
}

/// Rough rendered width, good enough to keep labels from overlapping.
fn estimate_width(text: &str, font_size: f32) -> f32 {
    text.chars().map(|c| if c.is_ascii() { 0.56 } else { 1.0 }).sum::<f32>() * font_size
}

/// First words of a note that aren't tags, short enough to sit under a dot.
fn note_label(content: &str) -> String {
    if let Some(title) = crate::core::frontmatter::title(content) {
        return parse::truncate_chars(&title, 16);
    }
    let plain = parse::plain_text(content);
    let text = plain
        .lines()
        .map(|line| line.split_whitespace().filter(|w| !w.starts_with('#')).collect::<Vec<_>>().join(" "))
        .find(|line| !line.is_empty())
        .unwrap_or_else(|| plain.lines().next().unwrap_or("").trim().to_string());
    parse::truncate_chars(&text, 16)
}

impl Render for GraphPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let notes = self.meta.iter().filter(|m| matches!(m.key, NodeKey::Note(_))).count();
        let tags = self.meta.len() - notes;
        let links = self.sim.as_ref().map(|s| s.links.len()).unwrap_or(0);
        let canvas = self.render_canvas(window, cx);
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .px_6()
                    .pt_4()
                    .pb_3()
                    .gap_3()
                    .child(h_flex().flex_shrink_0().gap_1().children(sidebar_toggle(cx)).child(div().text_lg().font_semibold().whitespace_nowrap().child(t!("关系图谱"))))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(t!("{} 条笔记 · {} 个标签 · {} 条连接", notes, tags, links)),
                    )
                    .child(div().w(px(180.)).flex_shrink_0().child(Input::new(&self.search).small().cleanable(true).prefix(Icon::new(Lucide::Search).small().text_color(theme.muted_foreground))))
                    .child(div().flex_shrink_0().child(Switch::new("graph-tags").small().label(t!("标签")).checked(self.show_tags).on_change(cx.listener(|this, checked: &bool, _, cx| {
                        this.show_tags = *checked;
                        this.rebuild(cx);
                    }))))
                    .child(div().flex_shrink_0().child(Switch::new("graph-orphans").small().label(t!("孤立笔记")).checked(self.show_orphans).on_change(cx.listener(|this, checked: &bool, _, cx| {
                        this.show_orphans = *checked;
                        this.rebuild(cx);
                    }))))
                    .child(div().flex_shrink_0().child(Switch::new("graph-semantic").small().label(t!("语义关联")).checked(self.include_semantic).on_change(cx.listener(|this, checked: &bool, _, cx| {
                        this.include_semantic = *checked;
                        if *checked {
                            this.compute_semantic(cx);
                        } else {
                            this.semantic_pairs.clear();
                            this.rebuild(cx);
                        }
                    })))),
            )
            .child(div().flex_1().min_h_0().border_t_1().border_color(theme.border).child(canvas))
    }
}
