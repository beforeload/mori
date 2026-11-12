//! Random walk as a branching tree on a pannable, zoomable canvas (flomo 随机漫步 3.0):
//! each step opens a column of semantically related memos; clicking one walks on,
//! clicking an earlier card walks back to it.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Bounds, Context, FontWeight, Hsla, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, PathBuilder, PinchEvent, Pixels,
    Point, Render, ScrollEvent, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, canvas, div, fill, point, prelude::FluentBuilder as _, px, size,
};

use crate::core::model::MemoId;
use crate::core::review::{self, SeededRng};
use crate::core::time;

use super::library::Library;
use super::theme::surfaces;
use super::{Command, Lucide, dispatch};

const CARD_W: f32 = 250.;
const CARD_H: f32 = 126.;
const CURRENT_H: f32 = 300.;
const COL_GAP: f32 = 96.;
const ROW_GAP: f32 = 14.;
const CHILDREN: usize = 7;

#[derive(Clone, Copy, PartialEq)]
enum NodeKind {
    /// A memo on the walked path; `level` is its index in the path.
    Path,
    Current,
    /// A candidate shown beside path node `parent`.
    Child,
}

#[derive(Clone)]
struct NodeBox {
    id: MemoId,
    x: f32,
    y: f32,
    h: f32,
    kind: NodeKind,
    /// Path index of the column this node hangs off (the parent), or own index for the root.
    parent_level: usize,
}

struct Drag {
    start: Point<Pixels>,
    start_pan: (f32, f32),
}

pub struct WalkPage {
    path: Vec<MemoId>,
    children: Vec<Vec<MemoId>>,
    collapsed: bool,
    zoom: f32,
    pan: (f32, f32),
    drag: Option<Drag>,
    drag_distance: Rc<Cell<f32>>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    center_pending: bool,
    rng: SeededRng,
    _subscriptions: Vec<Subscription>,
}

impl WalkPage {
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        let library = Library::global(cx);
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(42);
        let mut page = Self {
            path: Vec::new(),
            children: Vec::new(),
            collapsed: false,
            zoom: 0.8,
            pan: (0., 0.),
            drag: None,
            drag_distance: Rc::new(Cell::new(0.)),
            bounds: Rc::new(Cell::new(Bounds::default())),
            center_pending: true,
            rng: SeededRng::new(seed),
            _subscriptions: vec![cx.observe(&library, |this, library, cx| {
                let alive = this.path.last().is_some_and(|id| library.read(cx).store().get_active(id).is_some());
                if !alive {
                    this.start_random(cx);
                }
                cx.notify();
            })],
        };
        page.start_random(cx);
        page
    }

    fn related(&self, id: &str, cx: &mut Context<Self>) -> Vec<MemoId> {
        let library = Library::global(cx);
        let lib = library.read(cx);
        lib.index()
            .related(id, CHILDREN, |other| {
                !self.path.iter().any(|p| p == other) && lib.store().get_active(other).is_some()
            })
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    fn start_random(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<MemoId> = Library::global(cx).read(cx).store().active_memos().map(|m| m.id.clone()).collect();
        if ids.is_empty() {
            self.path.clear();
            self.children.clear();
            cx.notify();
            return;
        }
        let pick = ids[self.rng.below(ids.len())].clone();
        self.start_from(&pick, cx);
    }

    pub fn start_from(&mut self, id: &str, cx: &mut Context<Self>) {
        self.path = vec![id.to_string()];
        self.children = vec![self.related(id, cx)];
        self.center_pending = true;
        cx.notify();
    }

    fn select(&mut self, node: &NodeBox, cx: &mut Context<Self>) {
        if self.drag_distance.get() > 4. {
            return;
        }
        match node.kind {
            NodeKind::Current => {}
            NodeKind::Path => {
                let level = self.path.iter().position(|p| *p == node.id).unwrap_or(0);
                self.path.truncate(level + 1);
                self.children.truncate(level + 1);
            }
            NodeKind::Child => {
                self.path.truncate(node.parent_level + 1);
                self.children.truncate(node.parent_level + 1);
                self.path.push(node.id.clone());
                let next = self.related(&node.id, cx);
                self.children.push(next);
            }
        }
        self.center_pending = true;
        cx.notify();
    }

    fn layout(&self, current_h: f32) -> (Vec<NodeBox>, Vec<(usize, usize)>) {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        if self.path.is_empty() {
            return (nodes, edges);
        }
        let last = self.path.len() - 1;
        let height = |level: usize| if level == last { current_h } else { CARD_H };
        nodes.push(NodeBox {
            id: self.path[0].clone(),
            x: 0.,
            y: -height(0) / 2.,
            h: height(0),
            kind: if last == 0 { NodeKind::Current } else { NodeKind::Path },
            parent_level: 0,
        });
        let mut parent_box = 0;
        for level in 0..self.path.len() {
            let next_on_path = self.path.get(level + 1);
            let show_all = !self.collapsed || level == last;
            let kids: Vec<&MemoId> = self.children.get(level).map(|c| c.iter().collect()).unwrap_or_default();
            let mut column: Vec<&MemoId> = if show_all { kids } else { Vec::new() };
            if let Some(next) = next_on_path
                && !column.contains(&next)
            {
                column.insert(0, next);
            }
            if column.is_empty() {
                break;
            }
            let heights: Vec<f32> = column
                .iter()
                .map(|id| if Some(*id) == next_on_path { height(level + 1) } else { CARD_H })
                .collect();
            let total: f32 = heights.iter().sum::<f32>() + ROW_GAP * (heights.len() as f32 - 1.);
            let parent = &nodes[parent_box];
            let center = parent.y + parent.h / 2.;
            let x = (level as f32 + 1.) * (CARD_W + COL_GAP);
            let mut y = center - total / 2.;
            let mut next_box = None;
            for (id, h) in column.into_iter().zip(heights) {
                let on_path = Some(id) == next_on_path;
                let kind = if !on_path {
                    NodeKind::Child
                } else if level + 1 == last {
                    NodeKind::Current
                } else {
                    NodeKind::Path
                };
                nodes.push(NodeBox {
                    id: id.clone(),
                    x,
                    y,
                    h,
                    kind,
                    parent_level: level,
                });
                edges.push((parent_box, nodes.len() - 1));
                if on_path {
                    next_box = Some(nodes.len() - 1);
                }
                y += h + ROW_GAP;
            }
            match next_box {
                Some(b) => parent_box = b,
                None => break,
            }
        }
        (nodes, edges)
    }

    fn center_on_current(&mut self, nodes: &[NodeBox]) {
        let b = self.bounds.get();
        if b.size.width <= px(0.) {
            return;
        }
        let Some(current) = nodes.iter().find(|n| n.kind == NodeKind::Current) else {
            return;
        };
        let w = b.size.width.as_f32();
        let h = b.size.height.as_f32();
        let cx = current.x + CARD_W / 2.;
        let cy = current.y + current.h / 2.;
        self.pan = (w * 0.42 - cx * self.zoom, h * 0.5 - cy * self.zoom);
        self.center_pending = false;
    }

    fn zoom_at(&mut self, factor: f32, anchor: Option<Point<Pixels>>) {
        let b = self.bounds.get();
        let (ax, ay) = match anchor {
            Some(p) => ((p.x - b.origin.x).as_f32(), (p.y - b.origin.y).as_f32()),
            None => (b.size.width.as_f32() / 2., b.size.height.as_f32() / 2.),
        };
        let new_zoom = (self.zoom * factor).clamp(0.2, 2.0);
        let world_x = (ax - self.pan.0) / self.zoom;
        let world_y = (ay - self.pan.1) / self.zoom;
        self.zoom = new_zoom;
        self.pan = (ax - world_x * new_zoom, ay - world_y * new_zoom);
    }

    fn render_card(&self, node: &NodeBox, index: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let library = Library::global(cx);
        let lib = library.read(cx);
        let memo = lib.store().get(&node.id)?.clone();
        let z = self.zoom;
        let theme = cx.theme().clone();
        let s = surfaces(cx);
        let left = self.pan.0 + node.x * z;
        let top = self.pan.1 + node.y * z;
        let b = self.bounds.get();
        let (w, h) = (CARD_W * z, node.h * z);
        if left > b.size.width.as_f32() + 20. || top > b.size.height.as_f32() + 20. || left + w < -20. || top + h < -20. {
            return None;
        }
        let current = node.kind == NodeKind::Current;
        let on_path = node.kind == NodeKind::Path;
        let fs = |v: f32| px(v * z);
        let backlinks = lib.store().backlinks(&memo.id).len();
        let lines = if current {
            12
        } else if backlinks > 0 {
            3
        } else {
            4
        };
        let node_for_click = node.clone();
        let id = memo.id.clone();
        let action = |key: &'static str, icon: Lucide, label: &'static str, command: Command| {
            h_flex()
                .id(SharedString::from(format!("walk-act-{key}")))
                .gap(fs(4.))
                .px(fs(6.))
                .py(fs(3.))
                .rounded(fs(4.))
                .text_size(fs(12.))
                .text_color(theme.muted_foreground)
                .cursor_pointer()
                .hover(|d| d.bg(theme.muted).text_color(theme.foreground))
                .child(Icon::new(icon).size(fs(12.)))
                .child(label)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    dispatch(command.clone(), window, cx);
                })
        };
        Some(
            v_flex()
                .id(("walk-card", index))
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(w))
                .h(px(h))
                .p(fs(12.))
                .gap(fs(6.))
                .overflow_hidden()
                .rounded(fs(8.))
                .bg(s.card)
                .border_1()
                .border_color(if current {
                    s.accent
                } else if on_path {
                    s.accent.opacity(0.45)
                } else {
                    theme.border.opacity(0.7)
                })
                .when(current, |d| d.shadow_lg())
                .cursor_pointer()
                .hover(|d| d.border_color(s.accent.opacity(0.7)))
                .child(div().text_size(fs(11.)).text_color(theme.muted_foreground).child(time::fmt_minute(memo.created_at)))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .text_size(fs(13.))
                        .line_height(fs(20.))
                        .text_color(theme.foreground)
                        .line_clamp(lines)
                        .when(current, |d| d.font_weight(FontWeight::MEDIUM))
                        .child(memo.snippet(if current { 400 } else { 120 })),
                )
                .when(backlinks > 0 && !current, |d| {
                    d.child(div().text_size(fs(11.)).text_color(theme.muted_foreground).child(t!("{} 条批注", backlinks)))
                })
                .when(current, |d| {
                    d.child(
                        h_flex()
                            .gap(fs(2.))
                            .pt(fs(6.))
                            .border_t_1()
                            .border_color(theme.border.opacity(0.6))
                            .child(action("share", Lucide::Share2, t!("分享"), Command::ShareImage(id.clone())))
                            .child(action("annotate", Lucide::MessageSquare, t!("批注"), Command::Annotate(id.clone())))
                            .child(action("edit", Lucide::SquarePen, t!("编辑"), Command::Edit(id.clone())))
                            .child(action("trash", Lucide::Trash, t!("删除"), Command::Trash(id.clone()))),
                    )
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    if node_for_click.kind == NodeKind::Current && this.drag_distance.get() <= 4. {
                        dispatch(Command::OpenMemo(node_for_click.id.clone()), window, cx);
                    } else {
                        this.select(&node_for_click, cx);
                    }
                }))
                .into_any_element(),
        )
    }
}

impl WalkPage {
    fn show_postcard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let library = Library::global(cx);
        let lib = library.read(cx);
        let similarities: Vec<f32> =
            self.path.windows(2).map(|pair| lib.index().similarity(&pair[0], &pair[1])).collect();
        let summary = review::walk_summary(lib.store(), &self.path, &similarities);
        let stops: Vec<String> = self
            .path
            .iter()
            .filter_map(|id| lib.store().get(id))
            .map(|m| m.snippet(28))
            .collect();
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme().clone();
            let stat = |value: String, label: &'static str| {
                v_flex()
                    .flex_1()
                    .items_center()
                    .gap_0p5()
                    .child(div().text_2xl().font_semibold().child(value))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(label))
            };
            let leap = summary
                .biggest_leap
                .filter(|_| summary.steps > 2)
                .map(|step| t!("最远的一跳发生在第 {} 步，两条笔记几乎毫不相干，却被你连在了一起。", step));
            let again = view.clone();
            dialog
                .w(px(440.))
                .title(t!("漫步明信片"))
                .child(
                    v_flex()
                        .gap_4()
                        .child(
                            h_flex()
                                .py_3()
                                .rounded_lg()
                                .bg(theme.secondary)
                                .child(stat(summary.steps.to_string(), t!("步")))
                                .child(stat(summary.span_days.to_string(), t!("天跨度")))
                                .child(stat(summary.chars.to_string(), t!("字"))),
                        )
                        .child(div().text_sm().text_color(theme.muted_foreground).child(t!(
                            "从 {} 走到 {}。",
                            time::fmt_day(time::date_of(summary.earliest)),
                            time::fmt_day(time::date_of(summary.latest)),
                        )))
                        .when_some(leap, |this, leap| this.child(div().text_sm().child(leap)))
                        .child(v_flex().gap_1().children(stops.iter().enumerate().map(|(i, stop)| {
                            h_flex()
                                .gap_2()
                                .text_sm()
                                .child(
                                    div()
                                        .w_5()
                                        .text_color(theme.primary)
                                        .font_semibold()
                                        .child(format!("{}", i + 1)),
                                )
                                .child(div().flex_1().truncate().child(stop.clone()))
                        })))
                        .child(
                            h_flex()
                                .justify_end()
                                .gap_2()
                                .child(Button::new("postcard-again").label(t!("再走一次")).on_click(move |_, window, cx| {
                                    let _ = again.update(cx, |this, cx| this.start_random(cx));
                                    window.close_dialog(cx);
                                }))
                                .child(Button::new("postcard-close").primary().label(t!("结束漫步")).on_click(
                                    |_, window, cx| {
                                        window.close_dialog(cx);
                                        dispatch(Command::CloseOverlay, window, cx);
                                    },
                                )),
                        ),
                )
        });
    }
}

/// Height of the focused card: date, tags, up to 12 wrapped lines of body, action row.
fn current_height(memo: &crate::core::model::Memo) -> f32 {
    let per_line = ((CARD_W - 24.) / 13.).floor();
    let units: f32 = memo
        .content
        .lines()
        .map(|line| {
            let width: f32 = line.chars().map(|c| if c.is_ascii() { 0.55 } else { 1. }).sum();
            (width / per_line).ceil().max(1.)
        })
        .sum();
    let lines = units.clamp(1., 12.);
    (24. + 22. + lines * 20. + 44.).clamp(CARD_H, CURRENT_H)
}

fn tool_button(id: &'static str, icon: Lucide, tip: &'static str) -> Button {
    Button::new(id).icon(Icon::new(icon)).tooltip(tip)
}

impl Render for WalkPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.bounds.get().size.width <= px(0.) {
            self.bounds.set(Bounds::new(point(px(0.), px(0.)), window.viewport_size()));
        }
        let current_h = self
            .path
            .last()
            .and_then(|id| Library::global(cx).read(cx).store().get(id).map(current_height))
            .unwrap_or(CARD_H);
        let (nodes, edges) = self.layout(current_h);
        if self.center_pending {
            self.center_on_current(&nodes);
        }
        let theme = cx.theme().clone();
        let s = surfaces(cx);
        let bounds_cell = self.bounds.clone();
        let zoom = self.zoom;
        let pan = self.pan;
        let dot_color: Hsla = theme.muted_foreground.opacity(if theme.is_dark() { 0.22 } else { 0.28 });
        let line_color: Hsla = theme.muted_foreground.opacity(0.55);
        let edge_geometry: Vec<((f32, f32), (f32, f32))> = edges
            .iter()
            .map(|&(p, c)| {
                let parent = &nodes[p];
                let child = &nodes[c];
                (
                    (parent.x + CARD_W, parent.y + parent.h / 2.),
                    (child.x, child.y + child.h / 2.),
                )
            })
            .collect();
        let cards: Vec<AnyElement> = nodes
            .iter()
            .enumerate()
            .filter_map(|(i, node)| self.render_card(node, i, cx))
            .collect();
        let empty = nodes.is_empty();
        if self.center_pending {
            window.request_animation_frame();
        }

        div()
            .id("walk-canvas")
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(s.page)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, _, cx| {
                this.drag = Some(Drag { start: event.position, start_pan: this.pan });
                this.drag_distance.set(0.);
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if let Some(drag) = &this.drag {
                    if event.pressed_button != Some(MouseButton::Left) {
                        this.drag = None;
                        return;
                    }
                    let dx = (event.position.x - drag.start.x).as_f32();
                    let dy = (event.position.y - drag.start.y).as_f32();
                    this.pan = (drag.start_pan.0 + dx, drag.start_pan.1 + dy);
                    this.drag_distance.set((dx * dx + dy * dy).sqrt());
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| {
                this.drag = None;
                cx.notify();
            }))
            .on_scroll(cx.listener(|this, event: &ScrollEvent, window, cx| {
                let delta = event.delta.pixel_delta(window.line_height());
                if event.modifiers.platform || event.modifiers.control {
                    this.zoom_at((1. - delta.y.as_f32() * 0.004).clamp(0.8, 1.25), Some(event.position));
                } else {
                    this.pan = (this.pan.0 + delta.x.as_f32(), this.pan.1 + delta.y.as_f32());
                }
                cx.notify();
            }))
            .on_pinch(cx.listener(|this, event: &PinchEvent, _, cx| {
                this.zoom_at(1. + event.delta, Some(event.position));
                cx.notify();
            }))
            .child(
                canvas(
                    move |bounds, _, _| bounds_cell.set(bounds),
                    move |bounds, _, window, _| {
                        let step = (24. * zoom).max(10.);
                        let ox = pan.0.rem_euclid(step);
                        let oy = pan.1.rem_euclid(step);
                        let mut y = oy;
                        while y < bounds.size.height.as_f32() {
                            let mut x = ox;
                            while x < bounds.size.width.as_f32() {
                                window.paint_quad(fill(
                                    Bounds::new(point(bounds.origin.x + px(x), bounds.origin.y + px(y)), size(px(1.5), px(1.5))),
                                    dot_color,
                                ));
                                x += step;
                            }
                            y += step;
                        }
                        let at = |x: f32, y: f32| point(bounds.origin.x + px(pan.0 + x * zoom), bounds.origin.y + px(pan.1 + y * zoom));
                        let mut builder = PathBuilder::stroke(px(1.2));
                        for ((px1, py1), (cx2, cy2)) in &edge_geometry {
                            let spine = px1 + COL_GAP / 2.;
                            builder.move_to(at(*px1, *py1));
                            builder.line_to(at(spine, *py1));
                            builder.line_to(at(spine, *cy2));
                            builder.line_to(at(*cx2, *cy2));
                        }
                        if !edge_geometry.is_empty()
                            && let Ok(path) = builder.build()
                        {
                            window.paint_path(path, line_color);
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
            .children(cards)
            .when(empty, |this| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(theme.muted_foreground)
                        .child(t!("还没有笔记可以漫步")),
                )
            })
            .child(
                h_flex()
                    .absolute()
                    .top_3()
                    .right_4()
                    .gap_2()
                    .child(
                        tool_button("walk-center", Lucide::LocateFixed, t!("回到当前笔记"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.center_pending = true;
                                cx.notify();
                            })),
                    )
                    .child(
                        tool_button("walk-random", Lucide::Dices, t!("换个起点"))
                            .on_click(cx.listener(|this, _, _, cx| this.start_random(cx))),
                    )
                    .child(
                        tool_button(
                            "walk-collapse",
                            if self.collapsed { Lucide::Expand } else { Lucide::Shrink },
                            if self.collapsed { t!("展开全部分支") } else { t!("只看来时的路") },
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.collapsed = !this.collapsed;
                            this.center_pending = true;
                            cx.notify();
                        })),
                    )
                    .child(
                        tool_button("walk-postcard", Lucide::Flag, t!("结束并生成漫步明信片"))
                            .disabled(self.path.len() < 2)
                            .on_click(cx.listener(|this, _, window, cx| this.show_postcard(window, cx))),
                    )
                    .child(
                        tool_button("walk-close", Lucide::X, t!("关闭 Esc"))
                            .on_click(|_, window, cx| dispatch(Command::CloseOverlay, window, cx)),
                    ),
            )
            .child(
                h_flex()
                    .absolute()
                    .top_4()
                    .left(px(92.))
                    .gap_2()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(div().font_semibold().text_color(theme.foreground).child(t!("随机漫步")))
                    .child(t!("第 {} 步 · 点击右侧卡片继续前行，点击左侧卡片回到来时的路", self.path.len().max(1))),
            )
            .child(
                h_flex()
                    .absolute()
                    .bottom_4()
                    .left_4()
                    .gap_1()
                    .px_1()
                    .py_1()
                    .rounded_lg()
                    .bg(s.card)
                    .shadow_md()
                    .child(Button::new("walk-zoom-out").ghost().small().icon(Icon::new(Lucide::Minus)).on_click(cx.listener(|this, _, _, cx| {
                        this.zoom_at(1. / 1.2, None);
                        cx.notify();
                    })))
                    .child(div().w_12().flex().justify_center().text_sm().child(format!("{:.0}%", self.zoom * 100.)))
                    .child(Button::new("walk-zoom-in").ghost().small().icon(Icon::new(Lucide::Plus)).on_click(cx.listener(|this, _, _, cx| {
                        this.zoom_at(1.2, None);
                        cx.notify();
                    }))),
            )
            .child(
                h_flex()
                    .absolute()
                    .bottom_5()
                    .right_5()
                    .gap_1p5()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(Icon::new(Lucide::Info).xsmall())
                    .child(t!("关联由语义模型计算，仅供参考")),
            )
    }
}
