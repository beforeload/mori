//! Memo relationship graph: annotation/reference links plus optional semantic links.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AppContext as _, BorderStyle, Bounds, Context, Corners, Edges, InteractiveElement as _,
    IntoElement, MouseButton, MouseDownEvent, ParentElement as _, PathBuilder, Pixels, Render,
    Styled as _, Subscription, Task, Window, canvas, div, fill, point, prelude::FluentBuilder as _,
    px, quad, relative, size,
};

use crate::core::graph::{self, EdgeKind, GraphLayout};
use crate::core::model::MemoId;

use super::sidebar::sidebar_toggle;
use super::Lucide;
use super::library::Library;
use super::memo_card::mini_card;

const SEMANTIC_NODES: usize = 160;
const SEMANTIC_THRESHOLD: f32 = 0.16;

pub struct GraphPage {
    layout: Option<Rc<GraphLayout>>,
    computing: bool,
    include_semantic: bool,
    selected: Option<usize>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    _task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl GraphPage {
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        let library = Library::global(cx);
        let mut page = Self {
            layout: None,
            computing: false,
            include_semantic: false,
            selected: None,
            bounds: Rc::new(Cell::new(Bounds::default())),
            _task: None,
            _subscriptions: vec![cx.observe(&library, |_, _, cx| cx.notify())],
        };
        page.compute(cx);
        page
    }

    fn compute(&mut self, cx: &mut Context<Self>) {
        let library = Library::global(cx);
        let lib = library.read(cx);
        let store = lib.store();
        let mut ids: Vec<MemoId> = Vec::new();
        let mut position: HashMap<MemoId, usize> = HashMap::new();
        let mut edges: Vec<(usize, usize, EdgeKind)> = Vec::new();
        let mut index_of = |id: &str, ids: &mut Vec<MemoId>| -> usize {
            *position.entry(id.to_string()).or_insert_with(|| {
                ids.push(id.to_string());
                ids.len() - 1
            })
        };
        for memo in store.active_memos() {
            for r in &memo.refs {
                if r != &memo.id && store.get_active(r).is_some() {
                    let a = index_of(&memo.id, &mut ids);
                    let b = index_of(r, &mut ids);
                    edges.push((a, b, EdgeKind::Reference));
                }
            }
        }
        let semantic_candidates: Vec<MemoId> = if self.include_semantic {
            store.active_memos().take(SEMANTIC_NODES).map(|m| m.id.clone()).collect()
        } else {
            Vec::new()
        };
        let index = lib.index();
        self.computing = true;
        self.selected = None;
        cx.notify();
        self._task = Some(cx.spawn(async move |this, cx| {
            let layout = cx
                .background_spawn(async move {
                    let mut ids = ids;
                    let mut edges = edges;
                    let mut position: HashMap<MemoId, usize> =
                        ids.iter().enumerate().map(|(i, id)| (id.clone(), i)).collect();
                    for id in &semantic_candidates {
                        for (other, score) in index.related(id, 2, |o| semantic_candidates.iter().any(|c| c == o)) {
                            if score < SEMANTIC_THRESHOLD {
                                continue;
                            }
                            let mut get = |id: &str| -> usize {
                                *position.entry(id.to_string()).or_insert_with(|| {
                                    ids.push(id.to_string());
                                    ids.len() - 1
                                })
                            };
                            let (a, b) = (get(id), get(&other));
                            if !edges.iter().any(|&(x, y, _)| (x == a && y == b) || (x == b && y == a)) {
                                edges.push((a, b, EdgeKind::Semantic));
                            }
                        }
                    }
                    graph::layout(ids, edges)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.layout = Some(Rc::new(layout));
                this.computing = false;
                cx.notify();
            });
        }));
    }

    fn on_click(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some(layout) = self.layout.clone() else {
            return;
        };
        let b = self.bounds.get();
        let mut best: Option<(f32, usize)> = None;
        for (i, n) in layout.nodes.iter().enumerate() {
            let x = b.origin.x + b.size.width * n.x;
            let y = b.origin.y + b.size.height * n.y;
            let d = ((x - event.position.x).as_f32().powi(2) + (y - event.position.y).as_f32().powi(2)).sqrt();
            if d < 12.0 && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, i));
            }
        }
        self.selected = best.map(|(_, i)| i);
        cx.notify();
    }

    fn render_graph(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let library = Library::global(cx);
        let lib = library.read(cx);
        let Some(layout) = self.layout.clone().filter(|l| !l.nodes.is_empty()) else {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(theme.muted_foreground)
                .child(if self.computing { t!("正在计算关系图…") } else { t!("还没有笔记之间的批注或引用连接") })
                .when(!self.computing && !self.include_semantic, |this| {
                    this.child(div().text_xs().child(t!("在卡片菜单中点「批注」即可建立连接；也可以打开右上角的「语义关联」，看看笔记之间隐藏的联系。")))
                })
                .into_any_element();
        };
        let selected = self.selected;
        let neighbors: Vec<usize> = selected
            .map(|s| {
                layout
                    .edges
                    .iter()
                    .filter_map(|&(a, b, _)| if a == s { Some(b) } else if b == s { Some(a) } else { None })
                    .collect()
            })
            .unwrap_or_default();
        let mut labeled: Vec<usize> = (0..layout.nodes.len()).collect();
        labeled.sort_by(|a, b| layout.nodes[*b].degree.cmp(&layout.nodes[*a].degree));
        labeled.truncate(14);
        if let Some(s) = selected
            && !labeled.contains(&s)
        {
            labeled.push(s);
        }
        let labels: Vec<(usize, f32, f32, String)> = labeled
            .into_iter()
            .filter_map(|i| {
                let n = &layout.nodes[i];
                lib.store().get(&n.id).map(|m| (i, n.x, n.y, m.snippet(10)))
            })
            .collect();
        let bounds_cell = self.bounds.clone();
        let reference_color = theme.blue;
        let semantic_color = theme.green;
        let ring = theme.foreground;
        let bg = theme.background;
        let paint = layout.clone();
        let neighbor_set = neighbors.clone();
        div()
            .relative()
            .size_full()
            .rounded_lg()
            .overflow_hidden()
            .border_1()
            .border_color(theme.border)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| this.on_click(e, cx)))
            .child(
                canvas(
                    move |bounds, _, _| bounds_cell.set(bounds),
                    move |bounds, _, window, _| {
                        window.paint_quad(fill(bounds, bg));
                        let at = |x: f32, y: f32| point(bounds.origin.x + bounds.size.width * x, bounds.origin.y + bounds.size.height * y);
                        for kind in [EdgeKind::Semantic, EdgeKind::Reference] {
                            let mut builder = PathBuilder::stroke(px(if kind == EdgeKind::Reference { 1.4 } else { 0.9 }));
                            let mut any = false;
                            for &(a, b, k) in &paint.edges {
                                if k != kind {
                                    continue;
                                }
                                any = true;
                                builder.move_to(at(paint.nodes[a].x, paint.nodes[a].y));
                                builder.line_to(at(paint.nodes[b].x, paint.nodes[b].y));
                            }
                            if any && let Ok(path) = builder.build() {
                                let color = if kind == EdgeKind::Reference { reference_color.opacity(0.55) } else { semantic_color.opacity(0.35) };
                                window.paint_path(path, color);
                            }
                        }
                        for (i, node) in paint.nodes.iter().enumerate() {
                            let is_selected = selected == Some(i);
                            let is_neighbor = neighbor_set.contains(&i);
                            let r = px(3.0 + (node.degree.min(10) as f32) * 0.7) + if is_selected { px(2.) } else { px(0.) };
                            let has_ref = paint.edges.iter().any(|&(a, b, k)| k == EdgeKind::Reference && (a == i || b == i));
                            let color = if has_ref { reference_color } else { semantic_color };
                            let faded = selected.is_some() && !is_selected && !is_neighbor;
                            let c = at(node.x, node.y);
                            window.paint_quad(quad(
                                Bounds::new(point(c.x - r, c.y - r), size(r * 2., r * 2.)),
                                Corners::all(r),
                                color.opacity(if faded { 0.25 } else { 0.9 }),
                                Edges::all(if is_selected { px(2.) } else { px(0.) }),
                                ring,
                                BorderStyle::default(),
                            ));
                        }
                    },
                )
                .size_full(),
            )
            .children(labels.into_iter().map(|(i, x, y, text)| {
                let emphasized = selected == Some(i) || neighbors.contains(&i);
                div()
                    .absolute()
                    .left(relative(x))
                    .top(relative(y))
                    .child(
                        div()
                            .ml(px(8.))
                            .mt(px(-8.))
                            .px_1()
                            .rounded_sm()
                            .text_xs()
                            .bg(theme.background.opacity(0.8))
                            .text_color(if emphasized { theme.foreground } else { theme.muted_foreground })
                            .child(text),
                    )
            }))
            .into_any_element()
    }
}

impl Render for GraphPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let library = Library::global(cx);
        let lib = library.read(cx);
        let theme = cx.theme().clone();
        let (nodes, refs, semantic) = self
            .layout
            .as_ref()
            .map(|l| {
                (
                    l.nodes.len(),
                    l.edges.iter().filter(|e| e.2 == EdgeKind::Reference).count(),
                    l.edges.iter().filter(|e| e.2 == EdgeKind::Semantic).count(),
                )
            })
            .unwrap_or_default();
        let selected = self
            .selected
            .and_then(|i| self.layout.as_ref().map(|l| (i, l.clone())))
            .and_then(|(i, l)| {
                let memo = lib.store().get_active(&l.nodes[i].id)?.clone();
                let neighbors: Vec<_> = l
                    .edges
                    .iter()
                    .filter_map(|&(a, b, k)| if a == i { Some((b, k)) } else if b == i { Some((a, k)) } else { None })
                    .filter_map(|(j, k)| lib.store().get_active(&l.nodes[j].id).cloned().map(|m| (m, k)))
                    .collect();
                Some((memo, neighbors))
            });
        v_flex()
            .size_full()
            .px_6()
            .py_4()
            .gap_3()
            .child(
                h_flex()
                    .gap_3()
                    .child(h_flex().gap_1().children(sidebar_toggle(cx)).child(div().text_lg().font_semibold().child(t!("笔记关系图"))))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(t!("{} 条笔记 · {} 条批注连接 · {} 条语义关联", nodes, refs, semantic)),
                    )
                    .child(div().flex_1())
                    .child(
                        h_flex()
                            .gap_3()
                            .text_xs()
                            .child(h_flex().gap_1().child(div().w_4().h_0p5().bg(theme.blue)).child(t!("批注 / 引用")))
                            .child(h_flex().gap_1().child(div().w_4().h_0p5().bg(theme.green)).child(t!("语义关联"))),
                    )
                    .child(
                        Switch::new("graph-semantic")
                            .small()
                            .label(t!("语义关联"))
                            .checked(self.include_semantic)
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                this.include_semantic = *checked;
                                this.compute(cx);
                            })),
                    )
                    .child(
                        Button::new("graph-refresh")
                            .ghost()
                            .small()
                            .icon(Icon::new(Lucide::RefreshCw))
                            .loading(self.computing)
                            .on_click(cx.listener(|this, _, _, cx| this.compute(cx))),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_4()
                    .items_stretch()
                    .child(div().flex_1().min_w_0().child(self.render_graph(cx)))
                    .when_some(selected, |this, (memo, neighbors)| {
                        this.child(
                            v_flex()
                                .id("graph-side")
                                .w(px(300.))
                                .flex_shrink_0()
                                .gap_2()
                                .overflow_y_scrollbar()
                                .child(div().text_xs().text_color(theme.muted_foreground).child(t!("选中的笔记")))
                                .child(mini_card("graph-selected", &memo, None, cx))
                                .child(div().pt_2().text_xs().text_color(theme.muted_foreground).child(t!("相连的笔记 {}", neighbors.len())))
                                .children(neighbors.iter().map(|(m, k)| {
                                    mini_card("graph-neighbor", m, Some(if *k == EdgeKind::Reference { t!("批注").into() } else { t!("语义").into() }), cx)
                                })),
                        )
                    }),
            )
    }
}
