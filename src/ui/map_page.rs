//! Cognitive map: memos laid out by similarity, density rendered as a 3D contour terrain
//! whose peaks are the themes you keep returning to.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use chrono::NaiveDate;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    Anchor, AnyElement, AppContext as _, BorderStyle, Bounds, ClipboardItem, Context, Corners, Edges,
    Entity, Hsla, Image, ImageFormat, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, PathBuilder, PinchEvent,
    Pixels, Point, Render, ScrollEvent, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Task, TextAlign, TextRun, Window, canvas, div, fill, hsla, point,
    prelude::FluentBuilder as _, px, quad, size,
};

use crate::core::ai;
use crate::core::model::MemoId;
use crate::core::semantic::{self, MapLayout};
use crate::core::{share, time};

use super::library::Library;
use super::memo_card::mini_card;
use super::tasks::run_blocking;
use super::theme::surfaces;
use super::{Command, Lucide, dispatch};

const GRID_W: usize = 140;
const GRID_H: usize = 90;
const LEVELS: usize = 14;
const WORLD: f32 = 1060.;
const HEIGHT: f32 = 150.;
const PERSPECTIVE: f32 = 2200.;

struct Palette {
    name: &'static str,
    line: Hsla,
    glow: Hsla,
    dot: Hsla,
    label: Hsla,
}

fn palettes(dark: bool) -> [Palette; 3] {
    if dark {
        [
            Palette { name: t!("月光"), line: hsla(0., 0., 0.92, 1.), glow: hsla(44. / 360., 0.9, 0.62, 1.), dot: hsla(0., 0., 0.85, 1.), label: hsla(0., 0., 0.96, 1.) },
            Palette { name: t!("苔原"), line: hsla(150. / 360., 0.45, 0.62, 1.), glow: hsla(140. / 360., 0.7, 0.7, 1.), dot: hsla(150. / 360., 0.4, 0.75, 1.), label: hsla(140. / 360., 0.3, 0.92, 1.) },
            Palette { name: t!("暮色"), line: hsla(28. / 360., 0.75, 0.62, 1.), glow: hsla(16. / 360., 0.9, 0.6, 1.), dot: hsla(34. / 360., 0.8, 0.75, 1.), label: hsla(36. / 360., 0.9, 0.9, 1.) },
        ]
    } else {
        [
            Palette { name: t!("墨线"), line: hsla(0., 0., 0.25, 1.), glow: hsla(38. / 360., 0.95, 0.5, 1.), dot: hsla(0., 0., 0.35, 1.), label: hsla(0., 0., 0.12, 1.) },
            Palette { name: t!("苔原"), line: hsla(150. / 360., 0.55, 0.32, 1.), glow: hsla(140. / 360., 0.7, 0.42, 1.), dot: hsla(150. / 360., 0.5, 0.35, 1.), label: hsla(150. / 360., 0.6, 0.18, 1.) },
            Palette { name: t!("暮色"), line: hsla(22. / 360., 0.7, 0.42, 1.), glow: hsla(10. / 360., 0.85, 0.5, 1.), dot: hsla(26. / 360., 0.7, 0.45, 1.), label: hsla(20. / 360., 0.7, 0.25, 1.) },
        ]
    }
}

/// Density terrain for the memos visible at the current point of the timeline.
struct Terrain {
    grid: Vec<f32>,
    max: f32,
    contours: Vec<Vec<semantic::Segment>>,
}

impl Terrain {
    fn build(points: &[(f32, f32)]) -> Self {
        let grid = semantic::density_grid(points, GRID_W, GRID_H, 0.032);
        let max = grid.iter().cloned().fold(0.0f32, f32::max);
        let contours = if max > 0.0 {
            (1..=LEVELS)
                .map(|i| semantic::marching_squares(&grid, GRID_W, GRID_H, max * (i as f32 / (LEVELS as f32 + 1.)).powf(1.2)))
                .collect()
        } else {
            Vec::new()
        };
        Self { grid, max, contours }
    }

    fn height_at(&self, x: f32, y: f32) -> f32 {
        if self.max <= 0.0 {
            return 0.0;
        }
        let gx = ((x * GRID_W as f32) as usize).min(GRID_W - 1);
        let gy = ((y * GRID_H as f32) as usize).min(GRID_H - 1);
        self.grid[gy * GRID_W + gx] / self.max
    }
}

#[derive(Clone, Copy)]
struct View {
    zoom: f32,
    azimuth: f32,
    tilt: f32,
    pan: (f32, f32),
}

impl View {
    /// Map coordinates (unit square) plus normalized height to screen offsets from the centre.
    fn project(&self, x: f32, y: f32, h: f32) -> (f32, f32, f32) {
        let wx = (x - 0.5) * WORLD;
        let wy = (y - 0.5) * WORLD * 0.66;
        let (sin_a, cos_a) = self.azimuth.sin_cos();
        let rx = wx * cos_a - wy * sin_a;
        let ry = wx * sin_a + wy * cos_a;
        let (sin_t, cos_t) = self.tilt.sin_cos();
        let z = h.powf(0.85) * HEIGHT;
        // Points nearer the viewer (larger ry once tilted) appear slightly larger.
        let f = PERSPECTIVE / (PERSPECTIVE - ry * sin_t).max(400.);
        let sx = self.pan.0 + self.zoom * rx * f;
        let sy = self.pan.1 + self.zoom * (ry * cos_t - z * sin_t) * f;
        (sx, sy, f)
    }
}

type Polyline = Vec<((f32, f32), (f32, f32))>;

/// Everything painted for one frame, in screen pixels relative to the canvas origin.
struct Scene {
    lines: Vec<(Polyline, Hsla, f32)>,
    dots: Vec<((f32, f32), f32, Hsla)>,
    glows: Vec<((f32, f32), f32, Hsla)>,
    labels: Vec<((f32, f32), String, f32, Hsla)>,
}

struct Drag {
    start: Point<Pixels>,
    start_view: View,
    rotate: bool,
}

pub struct MapPage {
    layout: Option<Rc<MapLayout>>,
    computing: bool,
    computed_revision: Option<u64>,
    months: Vec<NaiveDate>,
    month_index: usize,
    playing: bool,
    peak_names: Vec<Option<String>>,
    naming: bool,
    selected_peak: Option<usize>,
    selected_point: Option<MemoId>,
    zoom: f32,
    azimuth: f32,
    tilt: f32,
    pan: (f32, f32),
    three_d: bool,
    palette: usize,
    show_help: bool,
    drag: Option<Drag>,
    drag_distance: f32,
    tilt_slider: Entity<SliderState>,
    month_slider: Entity<SliderState>,
    terrain: RefCell<Option<(Option<i64>, Rc<Terrain>)>>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    _task: Option<Task<()>>,
    _play_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl MapPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let library = Library::global(cx);
        let tilt_slider = cx.new(|_| SliderState::new().min(0.).max(70.).step(1.).default_value(52.));
        let month_slider = cx.new(|_| SliderState::new().min(0.).max(1.).step(1.).default_value(1.));
        let subscriptions = vec![
            cx.observe(&library, |this, _, cx| {
                if this.layout.is_none() && !this.computing {
                    this.compute(cx);
                }
                cx.notify();
            }),
            cx.subscribe_in(&tilt_slider, window, |this, _, event: &SliderEvent, _, cx| {
                if let SliderEvent::Change(SliderValue::Single(v)) | SliderEvent::Release(SliderValue::Single(v)) = event {
                    this.tilt = v.to_radians();
                    this.three_d = *v > 1.;
                    cx.notify();
                }
            }),
            cx.subscribe_in(&month_slider, window, |this, _, event: &SliderEvent, _, cx| {
                if let SliderEvent::Change(SliderValue::Single(v)) | SliderEvent::Release(SliderValue::Single(v)) = event {
                    this.month_index = (*v as usize).min(this.months.len().saturating_sub(1));
                    cx.notify();
                }
            }),
        ];
        let mut page = Self {
            layout: None,
            computing: false,
            computed_revision: None,
            months: Vec::new(),
            month_index: 0,
            playing: false,
            peak_names: Vec::new(),
            naming: false,
            selected_peak: None,
            selected_point: None,
            zoom: 1.0,
            azimuth: 0.0,
            tilt: 52f32.to_radians(),
            pan: (0., 0.),
            three_d: true,
            palette: 0,
            show_help: false,
            drag: None,
            drag_distance: 0.,
            tilt_slider,
            month_slider,
            terrain: RefCell::new(None),
            bounds: Rc::new(Cell::new(Bounds::default())),
            _task: None,
            _play_task: None,
            _subscriptions: subscriptions,
        };
        page.compute(cx);
        page
    }

    fn compute(&mut self, cx: &mut Context<Self>) {
        let library = Library::global(cx);
        let lib = library.read(cx);
        if lib.is_indexing() && lib.index().is_empty() {
            return;
        }
        let index = lib.index();
        let created: HashMap<String, i64> = lib.store().active_memos().map(|m| (m.id.clone(), m.created_at)).collect();
        let revision = lib.store().revision();
        let auto_name = lib.settings().ai.is_configured();
        self.computing = true;
        cx.notify();
        self._task = Some(cx.spawn(async move |this, cx| {
            let layout = cx.background_spawn(async move { semantic::compute_map(&index, &created) }).await;
            let _ = this.update(cx, |this, cx| {
                this.months.clear();
                if let Some(first) = layout.points.iter().map(|p| p.created_at).min() {
                    let mut month = time::month_start(time::date_of(first));
                    let last = time::month_start(time::today());
                    while month <= last {
                        this.months.push(month);
                        month = time::add_months(month, 1);
                    }
                }
                this.month_index = this.months.len().saturating_sub(1);
                this.peak_names = vec![None; layout.peaks.len()];
                this.selected_peak = None;
                this.selected_point = None;
                this.layout = Some(Rc::new(layout));
                this.terrain.replace(None);
                this.computing = false;
                this.computed_revision = Some(revision);
                cx.notify();
            });
            if auto_name {
                let _ = this.update(cx, |this, cx| this.name_peaks(cx));
            }
        }));
    }

    fn sync_month_slider(&self, window: &mut Window, cx: &mut Context<Self>) {
        let max = self.months.len().saturating_sub(1).max(1) as f32;
        let index = self.month_index as f32;
        self.month_slider.update(cx, |s, cx| {
            if s.max_value() != max {
                *s = SliderState::new().min(0.).max(max).step(1.).default_value(index);
                cx.notify();
            } else if s.value() != SliderValue::Single(index) {
                s.set_value(index, window, cx);
            }
        });
    }

    fn cutoff(&self) -> Option<i64> {
        if self.month_index + 1 >= self.months.len() {
            return None;
        }
        Some(time::start_of_day(time::add_months(self.months[self.month_index], 1)))
    }

    fn terrain(&self) -> Option<Rc<Terrain>> {
        let layout = self.layout.as_ref()?;
        let cutoff = self.cutoff();
        if let Some((key, terrain)) = self.terrain.borrow().as_ref()
            && *key == cutoff
        {
            return Some(terrain.clone());
        }
        let points: Vec<(f32, f32)> = layout
            .points
            .iter()
            .filter(|p| cutoff.is_none_or(|c| p.created_at < c))
            .map(|p| (p.x, p.y))
            .collect();
        let terrain = Rc::new(Terrain::build(&points));
        self.terrain.replace(Some((cutoff, terrain.clone())));
        Some(terrain)
    }

    fn view(&self, width: f32, height: f32) -> View {
        View {
            zoom: self.zoom * (width / 1100.).clamp(0.6, 1.6),
            azimuth: self.azimuth,
            tilt: if self.three_d { self.tilt } else { 0. },
            pan: (width / 2. + self.pan.0, height * 0.47 + self.pan.1),
        }
    }

    fn peak_label(&self, i: usize) -> String {
        self.peak_names
            .get(i)
            .cloned()
            .flatten()
            .or_else(|| self.layout.as_ref().map(|l| l.peaks[i].label.clone()))
            .unwrap_or_default()
    }

    fn visible_count(&self, peak: usize) -> usize {
        let (Some(layout), cutoff) = (self.layout.as_ref(), self.cutoff()) else {
            return 0;
        };
        layout
            .points
            .iter()
            .filter(|p| p.peak == Some(peak) && cutoff.is_none_or(|c| p.created_at < c))
            .count()
    }

    fn build_scene(&self, width: f32, height: f32, dark: bool) -> Option<Scene> {
        let layout = self.layout.clone()?;
        let terrain = self.terrain()?;
        let view = self.view(width, height);
        let palette = &palettes(dark)[self.palette % 3];
        let cutoff = self.cutoff();
        let mut scene = Scene { lines: Vec::new(), dots: Vec::new(), glows: Vec::new(), labels: Vec::new() };
        for (level, segments) in terrain.contours.iter().enumerate() {
            let frac = (level as f32 + 1.) / (LEVELS as f32 + 1.);
            let h = frac.powf(1.2);
            let projected: Vec<((f32, f32), (f32, f32))> = segments
                .iter()
                .map(|((x1, y1), (x2, y2))| {
                    let a = view.project(*x1, *y1, h);
                    let b = view.project(*x2, *y2, h);
                    ((a.0, a.1), (b.0, b.1))
                })
                .collect();
            let alpha = 0.16 + 0.5 * frac;
            scene.lines.push((projected, palette.line.opacity(alpha), 0.7 + 0.5 * frac));
        }
        for p in layout.points.iter().filter(|p| cutoff.is_none_or(|c| p.created_at < c)) {
            let h = terrain.height_at(p.x, p.y);
            let (sx, sy, f) = view.project(p.x, p.y, h);
            let selected = self.selected_point.as_ref() == Some(&p.id);
            let in_peak = self.selected_peak.is_some() && p.peak == self.selected_peak;
            let r = if selected { 4.5 } else if in_peak { 2.4 } else { 1.4 } * f;
            let alpha = if self.selected_peak.is_some() && !in_peak { 0.25 } else { 0.7 };
            scene.dots.push(((sx, sy), r, if selected { palette.glow } else { palette.dot.opacity(alpha) }));
        }
        for (i, peak) in layout.peaks.iter().enumerate() {
            let count = self.visible_count(i);
            if count == 0 {
                continue;
            }
            let h = terrain.height_at(peak.x, peak.y);
            let (sx, sy, f) = view.project(peak.x, peak.y, h);
            let selected = self.selected_peak == Some(i);
            let strength = (count as f32 / 24.).clamp(0.35, 1.0);
            for (ring, alpha) in [(16., 0.06), (10., 0.12), (6., 0.25), (3., 0.9)] {
                scene.glows.push(((sx, sy), ring * f * (0.7 + 0.5 * strength), palette.glow.opacity(alpha * if selected { 1.4 } else { 1.0 })));
            }
            let font = (11. + 7. * strength) * f.clamp(0.8, 1.3);
            scene.labels.push(((sx, sy - 12. * f - font), self.peak_label(i), font, if selected { palette.glow } else { palette.label }));
        }
        Some(scene)
    }

    fn hit_test(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let b = self.bounds.get();
        let (w, h) = (b.size.width.as_f32(), b.size.height.as_f32());
        let mx = (position.x - b.origin.x).as_f32();
        let my = (position.y - b.origin.y).as_f32();
        let Some(layout) = self.layout.clone() else {
            return;
        };
        let Some(terrain) = self.terrain() else {
            return;
        };
        let view = self.view(w, h);
        let cutoff = self.cutoff();
        let dist = |(sx, sy): (f32, f32)| ((sx - mx).powi(2) + (sy - my).powi(2)).sqrt();
        let mut best_peak: Option<(f32, usize)> = None;
        for (i, peak) in layout.peaks.iter().enumerate() {
            if self.visible_count(i) == 0 {
                continue;
            }
            let (sx, sy, f) = view.project(peak.x, peak.y, terrain.height_at(peak.x, peak.y));
            let d = dist((sx, sy)).min(dist((sx, sy - 22. * f)));
            if d < 26. && best_peak.is_none_or(|(bd, _)| d < bd) {
                best_peak = Some((d, i));
            }
        }
        if let Some((_, i)) = best_peak {
            self.selected_peak = Some(i);
            self.selected_point = None;
            cx.notify();
            return;
        }
        let mut best: Option<(f32, &semantic::MapPoint)> = None;
        for p in layout.points.iter().filter(|p| cutoff.is_none_or(|c| p.created_at < c)) {
            let (sx, sy, _) = view.project(p.x, p.y, terrain.height_at(p.x, p.y));
            let d = dist((sx, sy));
            if d < 8. && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, p));
            }
        }
        self.selected_point = best.map(|(_, p)| p.id.clone());
        if let Some((_, p)) = best {
            self.selected_peak = p.peak;
        } else {
            self.selected_peak = None;
        }
        cx.notify();
    }

    fn toggle_play(&mut self, cx: &mut Context<Self>) {
        if self.playing {
            self.playing = false;
            self._play_task = None;
            cx.notify();
            return;
        }
        if self.months.len() < 2 {
            return;
        }
        self.playing = true;
        if self.month_index + 1 >= self.months.len() {
            self.month_index = 0;
        }
        self._play_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(700)).await;
                let keep = this.update(cx, |this, cx| {
                    if this.month_index + 1 < this.months.len() {
                        this.month_index += 1;
                    }
                    let more = this.month_index + 1 < this.months.len();
                    this.playing = more;
                    cx.notify();
                    more
                });
                if !matches!(keep, Ok(true)) {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn name_peaks(&mut self, cx: &mut Context<Self>) {
        let Some(layout) = self.layout.clone() else {
            return;
        };
        let library = Library::global(cx);
        let lib = library.read(cx);
        let cfg = lib.settings().ai.clone();
        if !cfg.is_configured() || layout.peaks.is_empty() {
            return;
        }
        let groups: Vec<Vec<String>> = layout
            .peaks
            .iter()
            .map(|p| p.members.iter().filter_map(|id| lib.store().get(id)).map(|m| m.snippet(60)).collect())
            .collect();
        self.naming = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = run_blocking(move || ai::chat_complete(&cfg, &ai::peak_naming_messages(&groups))).await;
            let _ = this.update(cx, |this, cx| {
                this.naming = false;
                if let Some(Ok(reply)) = result {
                    for (i, name) in ai::parse_string_list(&reply).into_iter().enumerate() {
                        if let Some(slot) = this.peak_names.get_mut(i) {
                            *slot = Some(name.replace('_', " "));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn share(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dark = cx.theme().is_dark();
        let (w, h) = (1600., 1000.);
        let Some(scene) = self.build_scene(w, h, dark) else {
            return;
        };
        let month = self.months.get(self.month_index).map(|m| time::fmt_month(*m)).unwrap_or_default();
        let nickname = Library::global(cx).read(cx).settings().nickname.clone();
        let svg = scene_svg(&scene, w, h, dark, &t!("{}的认知地图 · {}", nickname, month));
        match share::render_png(&svg, 1.0).and_then(|png| share::save_png(&png, t!("认知地图")).map(|p| (png, p))) {
            Ok((png, path)) => {
                cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(ImageFormat::Png, png)));
                let reveal = path.clone();
                window.push_notification(
                    Notification::success(t!("已保存到 {}，并复制到剪贴板", path.display()))
                        .title(t!("分享图已生成"))
                        .action(move |_, _, cx| {
                            let reveal = reveal.clone();
                            Button::new("reveal-share").xsmall().label(t!("在访达中显示")).on_click(cx.listener(move |this, _, window, cx| {
                                cx.reveal_path(&reveal);
                                this.dismiss(window, cx);
                            }))
                        }),
                    cx,
                );
            }
            Err(e) => window.push_notification(Notification::warning(t!("生成分享图失败：{}", e)), cx),
        }
    }

    fn render_side(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let library = Library::global(cx);
        let lib = library.read(cx);
        let layout = self.layout.clone()?;
        let theme = cx.theme().clone();
        let s = surfaces(cx);
        let selected_point = self.selected_point.as_ref().and_then(|id| lib.store().get_active(id)).cloned();
        let peak = self.selected_peak.filter(|i| *i < layout.peaks.len());
        if peak.is_none() && selected_point.is_none() {
            return None;
        }
        let mut panel = v_flex()
            .id("map-side-scroll")
            .size_full()
            .p_3()
            .gap_2()
            .overflow_y_scrollbar()
            .child(
                h_flex()
                    .child(div().flex_1().font_semibold().child(match peak {
                        Some(i) => format!("▲ {}", self.peak_label(i)),
                        None => t!("选中的笔记").into(),
                    }))
                    .child(Button::new("map-side-close").ghost().xsmall().icon(Icon::new(Lucide::X)).on_click(cx.listener(|this, _, _, cx| {
                        this.selected_peak = None;
                        this.selected_point = None;
                        cx.notify();
                    }))),
            );
        if let Some(memo) = &selected_point {
            panel = panel.child(mini_card("map-point", memo, None, cx));
        }
        if let Some(i) = peak {
            let cutoff = self.cutoff();
            let mut members: Vec<_> = layout.peaks[i]
                .members
                .iter()
                .filter_map(|id| lib.store().get_active(id))
                .filter(|m| cutoff.is_none_or(|c| m.created_at < c))
                .collect();
            members.sort_by_key(|m| m.created_at);
            let ids: Vec<MemoId> = members.iter().map(|m| m.id.clone()).collect();
            let label = self.peak_label(i);
            let span = match (members.first(), members.last()) {
                (Some(a), Some(b)) => format!("{} → {}", time::fmt_date(a.created_at), time::fmt_date(b.created_at)),
                _ => String::new(),
            };
            panel = panel
                .child(div().text_xs().text_color(theme.muted_foreground).child(t!("主题回顾 · {} 条笔记 · {}", members.len(), span)))
                .child(Button::new("peak-insight").outline().small().icon(Icon::new(Lucide::Sparkles)).label(t!("AI 洞察这座山峰")).on_click(move |_, window, cx| {
                    dispatch(Command::InsightMemos(ids.clone(), t!("山峰「{}」", label)), window, cx)
                }))
                .children(members.iter().map(|m| mini_card("peak-member", m, None, cx)));
        }
        Some(
            div()
                .id("map-side")
                .absolute()
                .top(px(64.))
                .right(px(76.))
                .bottom(px(84.))
                .w(px(300.))
                .rounded_xl()
                .overflow_hidden()
                .bg(s.card.opacity(0.96))
                .border_1()
                .border_color(theme.border)
                .shadow_lg()
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_scroll(|_, _, cx| cx.stop_propagation())
                .child(panel)
                .into_any_element(),
        )
    }

    fn render_help(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let s = surfaces(cx);
        v_flex()
            .id("map-help")
            .absolute()
            .bottom(px(84.))
            .left(px(24.))
            .w(px(360.))
            .p_4()
            .gap_2()
            .rounded_xl()
            .bg(s.card.opacity(0.97))
            .border_1()
            .border_color(theme.border)
            .shadow_lg()
            .text_sm()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().font_semibold().child(t!("怎么看你的山峰")))
            .child(t!("· 等高线越密，围绕这个主题的笔记越多；孤峰说明这类笔记与其他内容关联不大。"))
            .child(t!("· 山峰来自笔记之间的语义相似度，独立于你的标签，常常能看到从未命名过的主题。"))
            .child(t!("· 点「记录足迹」进入时间的河流：每一帧是一个月，看看哪些山峰在长、哪些停了。"))
            .child(div().text_xs().text_color(theme.muted_foreground).child(t!("拖动平移 · ⇧+拖动旋转与俯仰 · 双指捏合或 ⌘+滚轮缩放 · 点击山峰查看主题回顾")))
            .into_any_element()
    }
}

fn hex(color: Hsla) -> (String, f32) {
    let rgb = color.to_rgb();
    (
        format!("#{:02x}{:02x}{:02x}", (rgb.r * 255.) as u8, (rgb.g * 255.) as u8, (rgb.b * 255.) as u8),
        rgb.a,
    )
}

fn scene_svg(scene: &Scene, width: f32, height: f32, dark: bool, title: &str) -> String {
    let mut out = String::new();
    let bg = if dark { "#161616" } else { "#f4f4f2" };
    let fg = if dark { "#d9d9d9" } else { "#333333" };
    out.push_str(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" font-family="Hiragino Sans GB, STHeiti, sans-serif"><rect width="{width}" height="{height}" fill="{bg}"/>"#
    ));
    for (segments, color, stroke) in &scene.lines {
        if segments.is_empty() {
            continue;
        }
        let (c, a) = hex(*color);
        let mut d = String::new();
        for ((x1, y1), (x2, y2)) in segments {
            d.push_str(&format!("M{x1:.1} {y1:.1}L{x2:.1} {y2:.1}"));
        }
        out.push_str(&format!(r#"<path d="{d}" stroke="{c}" stroke-opacity="{a:.2}" stroke-width="{:.2}" fill="none"/>"#, stroke * 1.2));
    }
    for ((x, y), r, color) in scene.dots.iter().chain(scene.glows.iter()) {
        let (c, a) = hex(*color);
        out.push_str(&format!(r#"<circle cx="{x:.1}" cy="{y:.1}" r="{:.1}" fill="{c}" fill-opacity="{a:.2}"/>"#, r * 1.2));
    }
    for ((x, y), text, size, color) in &scene.labels {
        let (c, _) = hex(*color);
        out.push_str(&format!(
            r#"<text x="{x:.1}" y="{:.1}" font-size="{:.1}" text-anchor="middle" fill="{c}" font-weight="600">{}</text>"#,
            y + size,
            size * 1.25,
            share::escape(text)
        ));
    }
    out.push_str(&format!(r#"<text x="48" y="72" font-size="30" fill="{fg}" font-weight="600">{}</text>"#, share::escape(title)));
    out.push_str(&format!(
        r#"<text x="{}" y="{}" font-size="20" fill="{fg}" fill-opacity="0.6" text-anchor="end">{}</text>"#,
        width - 48.,
        height - 40.,
        t!("Mori · 持续不断记录，意义自然浮现"),
    ));
    out.push_str("</svg>");
    out
}

impl Render for MapPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_month_slider(window, cx);
        let library = Library::global(cx);
        let stale = self.computed_revision.is_some_and(|r| r != library.read(cx).store().revision());
        let theme = cx.theme().clone();
        let s = surfaces(cx);
        let dark = theme.is_dark();
        if self.bounds.get().size.width <= px(0.) {
            self.bounds.set(Bounds::new(point(px(0.), px(0.)), window.viewport_size()));
        }
        let bounds = self.bounds.get();
        let scene = self.build_scene(bounds.size.width.as_f32(), bounds.size.height.as_f32(), dark);
        let bounds_cell = self.bounds.clone();
        let cross = theme.muted_foreground.opacity(if dark { 0.18 } else { 0.25 });
        let background = if dark { hsla(0., 0., 0.09, 1.) } else { s.page };
        let month_label = self.months.get(self.month_index).map(|m| time::fmt_month(*m)).unwrap_or_else(|| "—".into());
        let view = cx.entity().downgrade();
        let months = self.months.clone();
        let current_month = self.month_index;
        let palette_name = palettes(dark)[self.palette % 3].name;
        let degrees = self.azimuth.to_degrees();
        let control = |el: gpui_kit::Div| el.rounded_xl().bg(s.card.opacity(0.92)).border_1().border_color(theme.border).shadow_md();

        div()
            .id("map-canvas")
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(background)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, _, cx| {
                this.drag = Some(Drag {
                    start: event.position,
                    start_view: View { zoom: this.zoom, azimuth: this.azimuth, tilt: this.tilt, pan: this.pan },
                    rotate: event.modifiers.shift,
                });
                this.drag_distance = 0.;
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                let Some(drag) = &this.drag else {
                    return;
                };
                if event.pressed_button != Some(MouseButton::Left) {
                    this.drag = None;
                    return;
                }
                let dx = (event.position.x - drag.start.x).as_f32();
                let dy = (event.position.y - drag.start.y).as_f32();
                this.drag_distance = (dx * dx + dy * dy).sqrt();
                if drag.rotate {
                    this.azimuth = drag.start_view.azimuth + dx * 0.008;
                    this.tilt = (drag.start_view.tilt - dy * 0.006).clamp(0., 70f32.to_radians());
                    this.three_d = this.tilt > 0.01;
                    let degrees = this.tilt.to_degrees();
                    this.tilt_slider.update(cx, |s, cx| s.set_value(degrees, window, cx));
                } else {
                    this.pan = (drag.start_view.pan.0 + dx, drag.start_view.pan.1 + dy);
                }
                cx.notify();
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, event: &MouseUpEvent, _, cx| {
                let clicked = this.drag.is_some() && this.drag_distance < 4.;
                this.drag = None;
                if clicked {
                    this.hit_test(event.position, cx);
                }
            }))
            .on_scroll(cx.listener(|this, event: &ScrollEvent, window, cx| {
                let delta = event.delta.pixel_delta(window.line_height());
                if event.modifiers.platform || event.modifiers.control || !event.delta.precise() {
                    this.zoom = (this.zoom * (1. - delta.y.as_f32() * 0.003)).clamp(0.4, 4.0);
                } else {
                    this.pan = (this.pan.0 + delta.x.as_f32(), this.pan.1 + delta.y.as_f32());
                }
                cx.notify();
            }))
            .on_pinch(cx.listener(|this, event: &PinchEvent, _, cx| {
                this.zoom = (this.zoom * (1. + event.delta)).clamp(0.4, 4.0);
                cx.notify();
            }))
            .child(
                canvas(
                    move |bounds, _, _| bounds_cell.set(bounds),
                    move |bounds, _, window, cx| {
                        let o = bounds.origin;
                        let step = 34.;
                        let mut y = 17.;
                        while y < bounds.size.height.as_f32() {
                            let mut x = 17.;
                            while x < bounds.size.width.as_f32() {
                                window.paint_quad(fill(Bounds::new(point(o.x + px(x - 2.5), o.y + px(y - 0.4)), size(px(5.), px(0.8))), cross));
                                window.paint_quad(fill(Bounds::new(point(o.x + px(x - 0.4), o.y + px(y - 2.5)), size(px(0.8), px(5.))), cross));
                                x += step;
                            }
                            y += step;
                        }
                        let Some(scene) = scene else {
                            return;
                        };
                        let at = |(x, y): (f32, f32)| point(o.x + px(x), o.y + px(y));
                        for (segments, color, width) in &scene.lines {
                            if segments.is_empty() {
                                continue;
                            }
                            let mut builder = PathBuilder::stroke(px(*width));
                            for (a, b) in segments {
                                builder.move_to(at(*a));
                                builder.line_to(at(*b));
                            }
                            if let Ok(path) = builder.build() {
                                window.paint_path(path, *color);
                            }
                        }
                        for (center, r, color) in scene.dots.iter().chain(scene.glows.iter()) {
                            let r = px(*r);
                            let c = at(*center);
                            window.paint_quad(quad(
                                Bounds::new(point(c.x - r, c.y - r), size(r * 2., r * 2.)),
                                Corners::all(r),
                                *color,
                                Edges::all(px(0.)),
                                *color,
                                BorderStyle::default(),
                            ));
                        }
                        let font = window.text_style().font();
                        for (pos, text, size, color) in &scene.labels {
                            let run = TextRun {
                                len: text.len(),
                                font: gpui_kit::Font { weight: gpui_kit::FontWeight::SEMIBOLD, ..font.clone() },
                                color: *color,
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            };
                            let shaped = window.text_system().shape_line(SharedString::from(text.clone()), px(*size), &[run], None);
                            let width = shaped.width();
                            let _ = shaped.paint(at((pos.0, pos.1)) - point(width / 2., px(0.)), px(*size * 1.3), TextAlign::Left, None, window, cx);
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
            .when(self.layout.is_none(), |this| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(theme.muted_foreground)
                        .child(if self.computing { t!("正在绘制你的认知地图…") } else { t!("笔记太少，还画不出地图") }),
                )
            })
            .child(
                h_flex()
                    .absolute()
                    .top_4()
                    .left(px(92.))
                    .gap_2()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(div().font_semibold().text_color(theme.foreground).child(t!("认知地图")))
                    .child(t!(
                        "{} 座山峰 · {} 条笔记{}",
                        self.layout.as_ref().map(|l| l.peaks.len()).unwrap_or(0),
                        self.layout.as_ref().map(|l| l.points.len()).unwrap_or(0),
                        if self.naming { t!(" · AI 正在为山峰命名…") } else { "" }
                    )),
            )
            .child(
                v_flex()
                    .absolute()
                    .top_3()
                    .right_4()
                    .gap_3()
                    .items_center()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(Button::new("map-close").icon(Icon::new(Lucide::X)).tooltip(t!("关闭 Esc")).on_click(|_, window, cx| dispatch(Command::CloseOverlay, window, cx)))
                    .child(
                        control(v_flex())
                            .items_center()
                            .py_1()
                            .child(Button::new("map-zoom-in").ghost().xsmall().icon(Icon::new(Lucide::Plus)).on_click(cx.listener(|this, _, _, cx| {
                                this.zoom = (this.zoom * 1.2).min(4.0);
                                cx.notify();
                            })))
                            .child(div().text_xs().text_color(theme.muted_foreground).child(format!("{:.0}%", self.zoom * 100.)))
                            .child(Button::new("map-zoom-out").ghost().xsmall().icon(Icon::new(Lucide::Minus)).on_click(cx.listener(|this, _, _, cx| {
                                this.zoom = (this.zoom / 1.2).max(0.4);
                                cx.notify();
                            }))),
                    )
                    .child(
                        Button::new("map-2d")
                            .small()
                            .label(if self.three_d { "2D" } else { "3D" })
                            .tooltip(if self.three_d { t!("切换为俯视平面图") } else { t!("切换为立体地形") })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.three_d = !this.three_d;
                                if this.three_d && this.tilt < 0.1 {
                                    this.tilt = 52f32.to_radians();
                                }
                                let degrees = if this.three_d { this.tilt.to_degrees() } else { 0. };
                                this.tilt_slider.update(cx, |s, cx| s.set_value(degrees, window, cx));
                                cx.notify();
                            })),
                    )
                    .child(
                        control(div())
                            .h(px(110.))
                            .w(px(30.))
                            .py_2()
                            .flex()
                            .justify_center()
                            .child(Slider::new(&self.tilt_slider).vertical().disabled(!self.three_d)),
                    )
                    .child(
                        div()
                            .id("map-compass")
                            .size(px(40.))
                            .rounded_full()
                            .bg(s.card.opacity(0.92))
                            .border_1()
                            .border_color(theme.border)
                            .cursor_pointer()
                            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(t!("朝向 {}° · 点击回正，⇧+拖动地图旋转", format!("{degrees:.0}"))).build(window, cx))
                            .child(
                                canvas(
                                    |_, _, _| {},
                                    {
                                        let azimuth = self.azimuth;
                                        let north = theme.red;
                                        let south = theme.muted_foreground;
                                        move |bounds, _, window, _| {
                                            let c = bounds.center();
                                            let (sin, cos) = (-azimuth).sin_cos();
                                            let tip = |len: f32, side: f32| point(c.x + px(sin * len + cos * side), c.y + px(-cos * len + sin * side));
                                            let mut n = PathBuilder::fill();
                                            n.move_to(tip(13., 0.));
                                            n.line_to(tip(0., 3.5));
                                            n.line_to(tip(0., -3.5));
                                            n.close();
                                            if let Ok(p) = n.build() {
                                                window.paint_path(p, north);
                                            }
                                            let mut s = PathBuilder::fill();
                                            s.move_to(tip(-13., 0.));
                                            s.line_to(tip(0., 3.5));
                                            s.line_to(tip(0., -3.5));
                                            s.close();
                                            if let Ok(p) = s.build() {
                                                window.paint_path(p, south.opacity(0.6));
                                            }
                                        }
                                    },
                                )
                                .size_full(),
                            )
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.azimuth = 0.;
                                cx.notify();
                            })),
                    ),
            )
            .children(self.render_side(cx))
            .when(self.show_help, |this| this.child(self.render_help(cx)))
            .child(
                h_flex()
                    .absolute()
                    .bottom_5()
                    .left_0()
                    .right_0()
                    .justify_center()
                    .gap_3()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        control(h_flex())
                            .gap_1()
                            .px_1()
                            .py_1()
                            .child(Button::new("map-palette").ghost().small().icon(Icon::new(Lucide::Palette)).tooltip(t!("配色：{}", palette_name)).on_click(cx.listener(|this, _, _, cx| {
                                this.palette = (this.palette + 1) % 3;
                                cx.notify();
                            })))
                            .child(Button::new("map-help").ghost().small().icon(Icon::new(Lucide::CircleQuestionMark)).tooltip(t!("如何看懂认知地图")).on_click(cx.listener(|this, _, _, cx| {
                                this.show_help = !this.show_help;
                                cx.notify();
                            })))
                            .child(
                                Button::new("map-refresh")
                                    .ghost()
                                    .small()
                                    .icon(Icon::new(Lucide::RefreshCw))
                                    .loading(self.computing)
                                    .tooltip(if stale { t!("笔记有更新，重新绘制") } else { t!("重新绘制") })
                                    .on_click(cx.listener(|this, _, _, cx| this.compute(cx))),
                            ),
                    )
                    .child(
                        control(h_flex())
                            .gap_2()
                            .px_2()
                            .py_1()
                            .child(
                                Button::new("map-play")
                                    .ghost()
                                    .small()
                                    .w(px(96.))
                                    .icon(Icon::new(if self.playing { Lucide::Pause } else { Lucide::Play }))
                                    .label(if self.playing { t!("暂停") } else { t!("记录足迹") })
                                    .disabled(self.months.len() < 2)
                                    .on_click(cx.listener(|this, _, _, cx| this.toggle_play(cx))),
                            )
                            .child(
                                Button::new("map-month")
                                    .ghost()
                                    .small()
                                    .w(px(112.))
                                    .child(h_flex().gap_1().child(month_label).child(Icon::new(Lucide::ChevronDown).xsmall()))
                                    .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
                                        let menu = menu.scrollable(true).max_h(px(320.));
                                        months.iter().enumerate().rev().fold(menu, |menu, (i, m)| {
                                            let view = view.clone();
                                            menu.item(PopupMenuItem::new(time::fmt_month(*m)).checked(i == current_month).on_click(move |_, _, cx| {
                                                let _ = view.update(cx, |this, cx| {
                                                    this.month_index = i;
                                                    this.playing = false;
                                                    this._play_task = None;
                                                    cx.notify();
                                                });
                                            }))
                                        })
                                    }),
                            )
                            .child(div().w(px(200.)).child(Slider::new(&self.month_slider).disabled(self.months.len() < 2))),
                    )
                    .child(
                        control(h_flex())
                            .px_1()
                            .py_1()
                            .child(
                                Button::new("map-share")
                                    .ghost()
                                    .small()
                                    .icon(Icon::new(Lucide::ImageDown))
                                    .label(t!("分享图"))
                                    .disabled(self.layout.is_none())
                                    .on_click(cx.listener(|this, _, window, cx| this.share(window, cx))),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .absolute()
                    .bottom_6()
                    .right_5()
                    .gap_1p5()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(Icon::new(Lucide::Info).xsmall())
                    .child(t!("山峰名称由 AI 生成，仅供参考")),
            )
    }
}
