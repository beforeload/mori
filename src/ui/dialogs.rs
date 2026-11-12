use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{App, AppContext as _, Focusable as _, ParentElement as _, Styled as _, Window, div};

/// A single-line text prompt. `on_ok` returns an error message to keep the dialog open.
pub fn prompt_text(
    window: &mut Window,
    cx: &mut App,
    title: &str,
    description: &str,
    initial: &str,
    ok_label: &str,
    on_ok: impl Fn(String, &mut Window, &mut App) -> Result<(), String> + 'static,
) {
    let input = cx.new(|cx| InputState::new(window, cx).default_value(initial.to_string()));
    let initial_focus = Cell::new(true);
    let title = title.to_string();
    let description = description.to_string();
    let ok_label = ok_label.to_string();
    let on_ok = Rc::new(on_ok);
    let error: Rc<std::cell::RefCell<Option<String>>> = Rc::default();
    window.open_dialog(cx, move |dialog, window, cx| {
        if initial_focus.replace(false) {
            input.focus_handle(cx).focus(window, cx);
        }
        let input_for_ok = input.clone();
        let on_ok = on_ok.clone();
        let error_for_ok = error.clone();
        dialog
            .title(title.clone())
            .w(gpui_kit::px(440.))
            .child(
                v_flex()
                    .gap_2()
                    .when_desc(&description, cx)
                    .child(Input::new(&input))
                    .children(error.borrow().clone().map(|e| {
                        div().text_xs().text_color(cx.theme().danger).child(e)
                    })),
            )
            .on_ok(move |_, window, cx| {
                let value = input_for_ok.read(cx).value().trim().to_string();
                match on_ok(value, window, cx) {
                    Ok(()) => true,
                    Err(e) => {
                        *error_for_ok.borrow_mut() = Some(e);
                        window.refresh();
                        false
                    }
                }
            })
            .footer(
                DialogFooter::new()
                    .child(DialogClose::new().child(Button::new("cancel").outline().label(t!("取消"))))
                    .child(DialogAction::new().child(Button::new("ok").primary().label(ok_label.clone()))),
            )
    });
}

/// A labelled field for [`prompt_fields`]: (label, initial value, placeholder).
pub type Field<'a> = (&'a str, &'a str, &'a str);

/// Several single-line fields in one dialog; `on_ok` receives the trimmed values in order.
pub fn prompt_fields(
    window: &mut Window,
    cx: &mut App,
    title: &str,
    description: &str,
    fields: &[Field],
    ok_label: &str,
    on_ok: impl Fn(Vec<String>, &mut Window, &mut App) -> Result<(), String> + 'static,
) {
    let inputs: Vec<(String, gpui_kit::Entity<InputState>)> = fields
        .iter()
        .map(|(label, initial, placeholder)| {
            let state = cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(initial.to_string())
                    .placeholder(placeholder.to_string())
            });
            (label.to_string(), state)
        })
        .collect();
    let initial_focus = Cell::new(true);
    let title = title.to_string();
    let description = description.to_string();
    let ok_label = ok_label.to_string();
    let on_ok = Rc::new(on_ok);
    let error: Rc<std::cell::RefCell<Option<String>>> = Rc::default();
    window.open_dialog(cx, move |dialog, window, cx| {
        if initial_focus.replace(false)
            && let Some((_, first)) = inputs.first()
        {
            first.focus_handle(cx).focus(window, cx);
        }
        let states: Vec<_> = inputs.iter().map(|(_, s)| s.clone()).collect();
        let on_ok = on_ok.clone();
        let error_for_ok = error.clone();
        dialog
            .title(title.clone())
            .w(gpui_kit::px(440.))
            .child(
                v_flex()
                    .gap_3()
                    .when_desc(&description, cx)
                    .children(inputs.iter().map(|(label, state)| {
                        h_flex()
                            .gap_3()
                            .child(div().w(gpui_kit::px(48.)).text_sm().child(label.clone()))
                            .child(div().flex_1().child(Input::new(state)))
                    }))
                    .children(error.borrow().clone().map(|e| {
                        div().text_xs().text_color(cx.theme().danger).child(e)
                    })),
            )
            .on_ok(move |_, window, cx| {
                let values = states.iter().map(|s| s.read(cx).value().trim().to_string()).collect();
                match on_ok(values, window, cx) {
                    Ok(()) => true,
                    Err(e) => {
                        *error_for_ok.borrow_mut() = Some(e);
                        window.refresh();
                        false
                    }
                }
            })
            .footer(
                DialogFooter::new()
                    .child(DialogClose::new().child(Button::new("cancel").outline().label(t!("取消"))))
                    .child(DialogAction::new().child(Button::new("ok").primary().label(ok_label.clone()))),
            )
    });
}

trait DescExt {
    fn when_desc(self, description: &str, cx: &App) -> Self;
}

impl DescExt for gpui_kit::Div {
    fn when_desc(self, description: &str, cx: &App) -> Self {
        if description.is_empty() {
            self
        } else {
            self.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(description.to_string()),
            )
        }
    }
}

pub fn confirm(
    window: &mut Window,
    cx: &mut App,
    title: &str,
    description: &str,
    ok_label: &str,
    danger: bool,
    on_ok: impl Fn(&mut Window, &mut App) + 'static,
) {
    let title = title.to_string();
    let description = description.to_string();
    let ok_label = ok_label.to_string();
    let on_ok = Rc::new(on_ok);
    window.open_alert_dialog(cx, move |alert, _, _| {
        let on_ok = on_ok.clone();
        alert
            .title(title.clone())
            .description(description.clone())
            .confirm()
            .ok_text(ok_label.clone())
            .cancel_text(t!("取消"))
            .ok_variant(if danger { ButtonVariant::Danger } else { ButtonVariant::Primary })
            .on_ok(move |_, window, cx| {
                on_ok(window, cx);
                true
            })
    });
}
