use crate::platform_title_bar::PlatformTitleBar;
use gpui::{
    AnyWindowHandle, Context, Hsla, InteractiveElement, MouseButton, ParentElement, ScrollHandle,
    Styled, SystemWindowTab, SystemWindowTabController, Window, WindowId, actions, canvas, div,
};
use settings::{Settings, SettingsStore};

use theme::ThemeSettings;
use ui::{
    Color, ContextMenu, DynamicSpacing, IconButton, IconButtonShape, IconName, IconSize, Label,
    LabelSize, h_flex, prelude::*, right_click_menu,
};
use workspace::{
    CloseWindow, ItemSettings, Workspace, WorkspaceSettings,
    item::{ClosePosition, ShowCloseButton},
};

actions!(
    window,
    [
        ShowNextWindowTab,
        ShowPreviousWindowTab,
        MergeAllWindows,
        MoveTabToNewWindow
    ]
);

const TAB_DRAG_MIN_WIDTH_REM: f32 = 10.0;
const TAB_CONTAINER_MIN_WIDTH_REM: f32 = 8.0;

#[derive(Clone)]
pub struct DraggedWindowTab {
    pub id: WindowId,
    pub ix: usize,
    pub handle: AnyWindowHandle,
    pub title: String,
    pub width: Pixels,
    pub is_active: bool,
    pub active_background_color: Hsla,
    pub inactive_background_color: Hsla,
}

pub struct SystemWindowTabs {
    tab_bar_scroll_handle: ScrollHandle,
    measured_tab_width: Pixels,
    last_measured_tab_count: usize,
    last_measured_tab_bar_width: Pixels,
    last_dragged_tab: Option<DraggedWindowTab>,
    cached_tabs: Vec<SystemWindowTab>,
    cached_tabs_revision: Option<u64>,
    cached_fallback_title: Option<String>,
}

impl SystemWindowTabs {
    pub fn new() -> Self {
        Self {
            tab_bar_scroll_handle: ScrollHandle::new(),
            measured_tab_width: px(0.),
            last_measured_tab_count: 0,
            last_measured_tab_bar_width: px(0.),
            last_dragged_tab: None,
            cached_tabs: Vec::new(),
            cached_tabs_revision: None,
            cached_fallback_title: None,
        }
    }

    fn refresh_cached_tabs(&mut self, window: &Window, cx: &App) {
        let window_id = window.window_handle().window_id();
        let controller = cx.global::<SystemWindowTabController>();
        let tabs_revision = controller.tabs_revision();

        if let Some(tabs) = controller.tabs(window_id) {
            if self.cached_tabs_revision != Some(tabs_revision) || self.cached_tabs.is_empty() {
                self.cached_tabs = tabs.clone();
                self.cached_tabs_revision = Some(tabs_revision);
                self.cached_fallback_title = None;
            }
            return;
        }

        let window_title = window.window_title();
        let needs_refresh = self.cached_tabs_revision != Some(tabs_revision)
            || self.cached_tabs.is_empty()
            || self
                .cached_fallback_title
                .as_deref()
                .is_some_and(|title| title != window_title.as_str());
        if needs_refresh {
            let shared_title = SharedString::from(window_title.clone());
            self.cached_tabs = vec![SystemWindowTab::new(shared_title, window.window_handle())];
            self.cached_tabs_revision = Some(tabs_revision);
            self.cached_fallback_title = Some(window_title);
        }
    }

    /// Returns true if workspace tabs should be shown in the title bar.
    /// This hides the embedded tab bar when the system tabs feature is off
    /// or when there is only a single workspace tab.
    pub fn should_show_embedded_tabs(window: &Window, cx: &App) -> bool {
        let use_system_window_tabs = WorkspaceSettings::get_global(cx).use_system_window_tabs;
        if !use_system_window_tabs {
            return false;
        }

        let controller = cx.global::<SystemWindowTabController>();
        let tabs = controller.tabs(window.window_handle().window_id());
        let tab_count = tabs.map(|t| t.len()).unwrap_or(1);

        // Show tabs when the setting is enabled and there's more than 1 tab
        tab_count > 1
    }

    pub fn init(cx: &mut App) {
        let mut was_use_system_window_tabs =
            WorkspaceSettings::get_global(cx).use_system_window_tabs;

        cx.observe_global::<SettingsStore>(move |cx| {
            let use_system_window_tabs = WorkspaceSettings::get_global(cx).use_system_window_tabs;
            if use_system_window_tabs == was_use_system_window_tabs {
                return;
            }
            was_use_system_window_tabs = use_system_window_tabs;

            let tabbing_identifier = if use_system_window_tabs {
                Some(String::from("zed"))
            } else {
                None
            };

            if use_system_window_tabs {
                SystemWindowTabController::init(cx);
            }

            cx.windows().iter().for_each(|handle| {
                let _ = handle.update(cx, |_, window, cx| {
                    window.set_tabbing_identifier(tabbing_identifier.clone());
                    if use_system_window_tabs {
                        let tabs = if let Some(tabs) = window.tabbed_windows() {
                            tabs
                        } else {
                            vec![SystemWindowTab::new(
                                SharedString::from(window.window_title()),
                                window.window_handle(),
                            )]
                        };

                        SystemWindowTabController::add_tab(cx, handle.window_id(), tabs);
                    }
                });
            });
        })
        .detach();

        cx.observe_new(|workspace: &mut Workspace, _, _| {
            workspace.register_action_renderer(|div, _, window, cx| {
                let window_id = window.window_handle().window_id();
                let controller = cx.global::<SystemWindowTabController>();

                let tab_groups = controller.tab_groups();
                let tabs = controller.tabs(window_id);
                let Some(tabs) = tabs else {
                    return div;
                };

                div.when(tabs.len() > 1, |div| {
                    div.on_action(move |_: &ShowNextWindowTab, window, cx| {
                        SystemWindowTabController::select_next_tab(
                            cx,
                            window.window_handle().window_id(),
                        );
                    })
                    .on_action(move |_: &ShowPreviousWindowTab, window, cx| {
                        SystemWindowTabController::select_previous_tab(
                            cx,
                            window.window_handle().window_id(),
                        );
                    })
                    .on_action(move |_: &MoveTabToNewWindow, window, cx| {
                        SystemWindowTabController::move_tab_to_new_window(
                            cx,
                            window.window_handle().window_id(),
                        );
                        window.move_tab_to_new_window();
                    })
                })
                .when(tab_groups.len() > 1, |div| {
                    div.on_action(move |_: &MergeAllWindows, window, cx| {
                        SystemWindowTabController::merge_all_windows(
                            cx,
                            window.window_handle().window_id(),
                        );
                        window.merge_all_windows();
                    })
                })
            });
        })
        .detach();
    }

    fn render_tab(
        &self,
        ix: usize,
        item: &SystemWindowTab,
        active_background_color: Hsla,
        inactive_background_color: Hsla,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let entity = cx.entity();
        let settings = ItemSettings::get_global(cx);
        let close_side = &settings.close_position;
        let show_close_button = &settings.show_close_button;

        let rem_size = window.rem_size();
        let tab_height = PlatformTitleBar::height(window);
        let width = self
            .measured_tab_width
            .max(rem_size * TAB_DRAG_MIN_WIDTH_REM);
        let item_id = item.id;
        let item_handle = item.handle;
        let title = item.title.clone();
        let title_string = title.to_string();
        let is_active = window.window_handle().window_id() == item_id;

        let label = Label::new(title)
            .size(LabelSize::Small)
            .truncate()
            .color(if is_active {
                Color::Default
            } else {
                Color::Muted
            });

        let tab = h_flex()
            .id(ix)
            .group("tab")
            .overflow_hidden()
            .h(tab_height)
            .relative()
            .px(DynamicSpacing::Base16.px(cx))
            .items_center()
            .justify_center()
            .border_l_1()
            .border_color(cx.theme().colors().border)
            .cursor_pointer()
            .on_drag(
                DraggedWindowTab {
                    id: item_id,
                    ix,
                    handle: item_handle,
                    title: title_string,
                    width,
                    is_active,
                    active_background_color,
                    inactive_background_color,
                },
                move |tab, _, _, cx| {
                    entity.update(cx, |this, _cx| {
                        this.last_dragged_tab = Some(tab.clone());
                    });
                    cx.new(|_| tab.clone())
                },
            )
            .drag_over::<DraggedWindowTab>({
                let tab_ix = ix;
                move |element, dragged_tab: &DraggedWindowTab, _, cx| {
                    let mut styled_tab = element
                        .bg(cx.theme().colors().drop_target_background)
                        .border_color(cx.theme().colors().drop_target_border)
                        .border_0();

                    if tab_ix < dragged_tab.ix {
                        styled_tab = styled_tab.border_l_2();
                    } else if tab_ix > dragged_tab.ix {
                        styled_tab = styled_tab.border_r_2();
                    }

                    styled_tab
                }
            })
            .on_drop({
                let tab_ix = ix;
                cx.listener(move |this, dragged_tab: &DraggedWindowTab, _window, cx| {
                    this.last_dragged_tab = None;
                    Self::handle_tab_drop(dragged_tab, tab_ix, cx);
                })
            })
            .on_click(move |_, _, cx| {
                let _ = item_handle.update(cx, |_, window, _| {
                    window.activate_window();
                });
            })
            .on_mouse_up(MouseButton::Middle, move |_, window, cx| {
                if item_handle.window_id() == window.window_handle().window_id() {
                    window.dispatch_action(Box::new(CloseWindow), cx);
                } else {
                    let _ = item_handle.update(cx, |_, window, cx| {
                        window.dispatch_action(Box::new(CloseWindow), cx);
                    });
                }
            })
            .child(label)
            .map(|this| match show_close_button {
                ShowCloseButton::Hidden => this,
                _ => this.child(
                    h_flex()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .w_4()
                        .justify_center()
                        .map(|this| match close_side {
                            ClosePosition::Left => this.left_1(),
                            ClosePosition::Right => this.right_1(),
                        })
                        .child(
                            IconButton::new("close", IconName::Close)
                                .shape(IconButtonShape::Square)
                                .icon_color(Color::Muted)
                                .icon_size(IconSize::XSmall)
                                .on_click({
                                    move |_, window, cx| {
                                        if item_handle.window_id()
                                            == window.window_handle().window_id()
                                        {
                                            window.dispatch_action(Box::new(CloseWindow), cx);
                                        } else {
                                            let _ = item_handle.update(cx, |_, window, cx| {
                                                window.dispatch_action(Box::new(CloseWindow), cx);
                                            });
                                        }
                                    }
                                })
                                .map(|this| match show_close_button {
                                    ShowCloseButton::Hover => this.visible_on_hover("tab"),
                                    _ => this,
                                }),
                        ),
                ),
            })
            .into_any();

        let menu = right_click_menu(ix)
            .trigger(|_, _, _| tab)
            .menu(move |window, cx| {
                let focus_handle = cx.focus_handle();

                ContextMenu::build(window, cx, move |mut menu, _window_, _cx| {
                    menu = menu.entry("Close Tab", None, move |window, cx| {
                        let controller = cx.global::<SystemWindowTabController>();
                        let tabs = controller.tabs(item_id).cloned().unwrap_or_default();
                        Self::handle_right_click_action(
                            cx,
                            window,
                            &tabs,
                            |tab| tab.id == item_id,
                            |window, cx| {
                                window.dispatch_action(Box::new(CloseWindow), cx);
                            },
                        );
                    });

                    menu = menu.entry("Close Other Tabs", None, move |window, cx| {
                        let controller = cx.global::<SystemWindowTabController>();
                        let tabs = controller.tabs(item_id).cloned().unwrap_or_default();
                        Self::handle_right_click_action(
                            cx,
                            window,
                            &tabs,
                            |tab| tab.id != item_id,
                            |window, cx| {
                                window.dispatch_action(Box::new(CloseWindow), cx);
                            },
                        );
                    });

                    menu = menu.entry("Move Tab to New Window", None, move |window, cx| {
                        let controller = cx.global::<SystemWindowTabController>();
                        let tabs = controller.tabs(item_id).cloned().unwrap_or_default();
                        Self::handle_right_click_action(
                            cx,
                            window,
                            &tabs,
                            |tab| tab.id == item_id,
                            |window, cx| {
                                SystemWindowTabController::move_tab_to_new_window(
                                    cx,
                                    window.window_handle().window_id(),
                                );
                                window.move_tab_to_new_window();
                            },
                        );
                    });

                    menu = menu.entry("Show All Tabs", None, move |window, cx| {
                        let controller = cx.global::<SystemWindowTabController>();
                        let tabs = controller.tabs(item_id).cloned().unwrap_or_default();
                        Self::handle_right_click_action(
                            cx,
                            window,
                            &tabs,
                            |tab| tab.id == item_id,
                            |window, _cx| {
                                window.toggle_window_tab_overview();
                            },
                        );
                    });

                    menu.context(focus_handle)
                })
            });

        div()
            .flex_1()
            .h_full()
            .min_w(rem_size * TAB_CONTAINER_MIN_WIDTH_REM)
            .when(is_active, |this| this.bg(active_background_color))
            .when(!is_active, |this| this.bg(inactive_background_color))
            .border_l_1()
            .border_color(cx.theme().colors().border)
            .child(menu)
    }

    fn handle_tab_drop(dragged_tab: &DraggedWindowTab, ix: usize, cx: &mut Context<Self>) {
        SystemWindowTabController::update_tab_position(cx, dragged_tab.id, ix);
    }

    fn handle_right_click_action<F, P>(
        cx: &mut App,
        window: &mut Window,
        tabs: &[SystemWindowTab],
        predicate: P,
        mut action: F,
    ) where
        P: Fn(&SystemWindowTab) -> bool,
        F: FnMut(&mut Window, &mut App),
    {
        for tab in tabs {
            if predicate(tab) {
                if tab.id == window.window_handle().window_id() {
                    action(window, cx);
                } else {
                    let _ = tab.handle.update(cx, |_view, window, cx| {
                        action(window, cx);
                    });
                }
            }
        }
    }
}

impl Render for SystemWindowTabs {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let use_system_window_tabs = WorkspaceSettings::get_global(cx).use_system_window_tabs;
        let active_background_color = cx.theme().colors().tab_bar_background;
        let inactive_background_color = cx.theme().colors().title_bar_background;
        let entity = cx.entity();

        self.refresh_cached_tabs(window, cx);
        let tabs = self.cached_tabs.as_slice();
        let number_of_tabs = tabs.len().max(1);
        let tab_items = tabs
            .iter()
            .enumerate()
            .map(|(ix, item)| {
                self.render_tab(
                    ix,
                    item,
                    active_background_color,
                    inactive_background_color,
                    window,
                    cx,
                )
            })
            .collect::<Vec<_>>();

        // Don't show if the feature is disabled or there's only one tab
        if !use_system_window_tabs || number_of_tabs <= 1 {
            self.last_measured_tab_count = if use_system_window_tabs {
                number_of_tabs
            } else {
                0
            };
            self.last_measured_tab_bar_width = px(0.);
            return h_flex().into_any_element();
        }

        // Render embedded in title bar (takes full height of parent, fills available width)
        h_flex()
            .flex_1()
            .h_full()
            .items_center()
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _event, window, cx| {
                    if let Some(tab) = this.last_dragged_tab.take() {
                        SystemWindowTabController::move_tab_to_new_window(cx, tab.id);
                        if tab.id == window.window_handle().window_id() {
                            window.move_tab_to_new_window();
                        } else {
                            let _ = tab.handle.update(cx, |_, window, _cx| {
                                window.move_tab_to_new_window();
                            });
                        }
                    }
                }),
            )
            .child(
                h_flex()
                    .id("window tabs")
                    .flex_1()
                    .h_full()
                    .items_center()
                    .overflow_x_scroll()
                    .track_scroll(&self.tab_bar_scroll_handle)
                    .children(tab_items)
                    .child(
                        canvas(
                            |_, _, _| (),
                            move |bounds, _, _, cx| {
                                let entity = entity.clone();
                                entity.update(cx, |this, cx| {
                                    let tab_bar_width = bounds.size.width;
                                    if number_of_tabs != this.last_measured_tab_count
                                        || tab_bar_width != this.last_measured_tab_bar_width
                                    {
                                        let width = tab_bar_width / number_of_tabs as f32;
                                        this.measured_tab_width = width;
                                        this.last_measured_tab_count = number_of_tabs;
                                        this.last_measured_tab_bar_width = tab_bar_width;
                                        cx.notify();
                                    }
                                });
                            },
                        )
                        .absolute()
                        .size_full(),
                    ),
            )
            .child(
                h_flex()
                    .h_full()
                    .items_center()
                    .px(DynamicSpacing::Base06.rems(cx))
                    .child(
                        IconButton::new("plus", IconName::Plus)
                            .icon_size(IconSize::Small)
                            .icon_color(Color::Muted)
                            .on_click(|_event, window, cx| {
                                window.dispatch_action(
                                    Box::new(zed_actions::OpenRecent {
                                        create_new_window: true,
                                    }),
                                    cx,
                                );
                            }),
                    ),
            )
            .into_any_element()
    }
}

impl Render for DraggedWindowTab {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        let ui_font = ThemeSettings::get_global(cx).ui_font.clone();
        let label = Label::new(self.title.clone())
            .size(LabelSize::Small)
            .truncate()
            .color(if self.is_active {
                Color::Default
            } else {
                Color::Muted
            });

        h_flex()
            .h(PlatformTitleBar::height(window))
            .w(self.width)
            .px(DynamicSpacing::Base16.px(cx))
            .justify_center()
            .bg(if self.is_active {
                self.active_background_color
            } else {
                self.inactive_background_color
            })
            .border_1()
            .border_color(cx.theme().colors().border)
            .font(ui_font)
            .child(label)
    }
}
