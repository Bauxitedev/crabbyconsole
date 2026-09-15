use std::{cell::RefCell, ops::Deref as _, time::Duration};

use crabbyconsole_misc::profile::Ms;
use godot::{
    classes::{RichTextLabel, ShaderMaterial, Time},
    global::remap,
    prelude::*,
};
use mini_moka::unsync::Cache;
use time::{OffsetDateTime, UtcOffset, macros::format_description};

use crate::gd::console::{
    CrabConsole,
    autocomplete::{AutocompleteSlot, AutocompleteType, SearchSlot, SuggestionSlots},
    eval::watch::{Watch, WatchResult, WatchSlotKind},
    job::{EvalClapError, JobError, JobEvent, JobResult},
    util::BoxedCache,
};

pub(super) const COLOR_PROP: Color = Color::CORNFLOWER_BLUE; // e.g. name, modulate
pub(super) const COLOR_METHOD: Color = Color::SPRING_GREEN; // e.g. queue_free, hide
pub(super) const COLOR_UTILITY_FUNC: Color = Color::LAWN_GREEN; // e.g. print, push_error, instance_from_id
pub(super) const COLOR_GLOBAL: Color = Color::PURPLE; // e.g. IP, Engine
pub(super) const COLOR_CLASS: Color = Color::HOT_PINK; // e.g. Node3D, 
pub(super) const COLOR_BUILT_IN_TYPE: Color = Color::YELLOW; // e.g. Color, String
pub(super) const COLOR_SIGNAL: Color = Color::ORANGE; // e.g. tree_exited
pub(super) const COLOR_MISC: Color = Color::DARK_GRAY;
pub(super) const COLOR_PREFIX: Color = Color::from_rgb(0.4, 0.4, 0.4);

pub(super) const COLOR_SUCCESS: Color = Color::SPRING_GREEN;
pub(super) const COLOR_FAILURE: Color = Color::ORANGE;

pub(super) const COLOR_TIMESTAMP: Color = Color::DARK_GRAY;

impl CrabConsole {
    pub(super) fn maybe_add_timestamp(&mut self) {
        if !self.timestamps_enabled {
            return;
        }

        let rich_text = &mut self.nodes.console_history;
        // Add timestamp
        rich_text.push_color(COLOR_TIMESTAMP);
        rich_text.add_text(&format!("[{}] ", current_local_time_hms()));
        rich_text.pop();
    }

    #[tracing::instrument(skip_all)]
    pub(super) fn update_watch_text(&mut self) {
        // Although this is called (almost) every frame, if you have a lot of watches, this method is surprisingly NOT the cpu bottleneck.
        // Instead it seems to be the rendering/layout of the RTL itself, according to samply.
        // So splitting it up will still be beneficial, if we can make it so we don't have to update the RTL every frame.
        // But definitely make a separate branch for that + a reliable test case, to be 100% sure splitting it up is actually faster when you have a lot of watches.

        let ticks = Time::singleton().get_ticks_msec();
        let time_secs = ticks as f64 / 1000.0; // rounded to nearest ms

        let mut watch_label = self.nodes.watch_label.clone(); // fast clone

        watch_label.clear(); // <-- TODO this is showing up in the profiler
        watch_label.add_text(&format!(
            "Watches [{}]: (- = sync, > = async, @ = signal)\n",
            self.watches.watches.len(),
        ));

        for (watch_name, Watch { slot, .. }) in self.watches.watches.iter() {
            let is_busy = slot.borrow().is_busy;

            // Draw a -, > or @ depending if this is sync, async, or a signal
            {
                let list_symbol = match slot.borrow().kind {
                    WatchSlotKind::Sync => '-',
                    WatchSlotKind::Async => '>',
                    WatchSlotKind::Signal => '@',
                };

                watch_label.push_color(if is_busy {
                    Color::INDIAN_RED
                } else {
                    Color::WHITE
                });
                watch_label.add_text(&list_symbol.to_string());
                watch_label.pop(); // pop color

                watch_label.add_text(" ");
            }

            // How long ago the result was calculated.
            let time = slot.borrow().result.as_ref().map(|result| result.timestamp);
            let brightness_in = time
                .map(|time| 1.0 / (time.elapsed().as_secs_f64() * 10. + 1.).powf(0.2))
                .unwrap_or(0.0); //1.0 = just arrived, 0.0 = arrived forever ago (exponential decay)
            let brightness_bgcolor = time
                .map(|time| 1.0 / (time.elapsed().as_secs_f64() * 4. + 1.))
                .unwrap_or(0.0);

            // Color the text based on how long ago it was calculated.
            let watch_color = match slot.borrow().kind {
                WatchSlotKind::Sync => COLOR_SUCCESS.lerp(Color::WHITE, brightness_in),
                WatchSlotKind::Async => COLOR_PROP.lerp(Color::WHITE, brightness_in),
                WatchSlotKind::Signal => fire_gradient(brightness_in as f32),
            };

            let bgcolor = Color::from_rgba(1., 1., 1., brightness_bgcolor as f32);
            let use_watch_underline = bgcolor.a > 0.2;

            if use_watch_underline {
                watch_label.push_underline_ex().color(bgcolor).done();
            }
            watch_label.push_color(watch_color);
            watch_label.add_text(watch_name);
            watch_label.pop(); // pop color
            if use_watch_underline {
                watch_label.pop(); // pop underline
            }

            watch_label.add_text(" = ");

            if let Some(WatchResult {
                result_stringified, ..
            }) = &slot.borrow().result
            {
                let lighten_factor = brightness_in * 0.5;

                match result_stringified {
                    Ok((res, typ)) => {
                        // Color the text based on how long ago it was calculated.
                        let brightness = remap(brightness_in, 0.0, 1.0, 0.8, 1.0);

                        watch_label.push_color(Color::from_hsv(0.0, 0.0, brightness));
                        res.add_to(watch_label.clone(), &mut self.job_result_cache);
                        watch_label.pop(); // pop color

                        watch_label
                            .push_color(Color::CYAN.with_alpha(0.5).lightened(lighten_factor));

                        watch_label.add_text(&format!(" [{}]", typ.fancy_type_name()));
                        watch_label.pop();
                    }
                    Err(err) => {
                        // Ensure every unique watch error is printed only once per 5 seconds at most, to avoid spam/lag.
                        if deduplicate_error(err) {
                            tracing::warn!(?err, "watch `{watch_name}` failed");
                        }

                        let error_color = Color::RED;

                        watch_label.push_color(error_color.lightened(lighten_factor));
                        watch_label.add_text(&match err.deref() {
                            e @ JobError::TimeoutAsync(_) | e @ JobError::TimeoutSignal(_) => {
                                e.to_string() // NOTE - watches no longer timeout, this branch can maybe be removed
                            }
                            JobError::EvalGodot(e) => {
                                format!("godot error: {e} - see stdout for details")
                            }

                            JobError::EvalClap(e) => {
                                format!("clap error: {e} - see stdout for details")
                            }
                        });
                        watch_label.pop();
                    }
                };
            } else {
                watch_label.push_color(Color::from_rgb(0.5, 0.5, 0.5));
                let period_count = ((time_secs * 3.).round() as usize) % 4;
                let periods: String = vec!['.'; period_count].into_iter().collect();
                watch_label.add_text(&format!("waiting for result{periods}"));
                watch_label.pop();
            }

            watch_label.newline();
        }
    }

    #[tracing::instrument(skip_all)]
    pub(super) fn update_remote_text(&mut self) {
        let ticks = Time::singleton().get_ticks_msec();
        let time_secs = ticks as f64 / 1000.0; // rounded to nearest ms
        let blink = (time_secs % 0.5) > 0.25;

        let mut remote_label = self.nodes.remote_label.clone(); // fast clone
        let server = &self.server;

        remote_label.clear();
        remote_label.add_text("Status: ");

        if let Some(server) = server {
            remote_label.push_color(if blink { Color::ORANGE } else { Color::WHITE });
            remote_label.add_text(&format!("online ({}:{})", server.ip, server.port));
            remote_label.pop();
        } else {
            remote_label.push_color(Color::GRAY);
            remote_label.add_text("offline");
            remote_label.pop();
        }

        remote_label.newline();

        if let Some(server) = server {
            let conns = &server.connections;
            remote_label.add_text(&format!("Connected clients ({}):\n", conns.len(),));

            for (ip, cl) in conns {
                remote_label.add_text("- ");

                remote_label.push_color(Color::YELLOW);
                remote_label.add_text(ip);
                remote_label.pop();

                remote_label.add_text(&format!(" (commands run: {})\n", cl.commands_run));
            }
        } else {
            remote_label.newline();
            remote_label.add_text("To start the remote console, run ");

            remote_label.push_color(Color::YELLOW);
            remote_label.add_text(":cons remote start");
            remote_label.pop();

            remote_label.add_text(".");
        }
    }

    /// Logs a `JobEvent` to the console.
    /// If ev == Done, it will return ev (this indicates no more events should come in)
    pub(super) fn apply_event(&mut self, ev: JobEvent) -> Option<JobResult> {
        let mut rich_text = self.nodes.console_history.clone();

        // NOTE - do NOT use rich_text.append_text(&result) - this uses bbtext, you want to escape the text first to avoid bbcode injection!
        // Use add_text instead!

        match ev {
            JobEvent::WaitingAsync => {
                self.maybe_add_timestamp();

                rich_text.push_color(Color::GRAY);
                rich_text.add_text("Waiting for async... (press Ctrl+C to cancel)");
                rich_text.pop();
                rich_text.newline();

                None
            }
            JobEvent::WaitingSignal => {
                self.maybe_add_timestamp();

                rich_text.push_color(Color::GRAY);
                rich_text.add_text("Waiting for signal... (press Ctrl+C to cancel)");
                rich_text.pop();
                rich_text.newline();

                None
            }
            JobEvent::Done(jr) => {
                let JobResult {
                    result,
                    execution_time,
                    async_wait_time,
                    signal_wait_time,
                } = &jr;
                match result {
                    Ok((result, result_type)) => {
                        self.maybe_add_timestamp();

                        // Add result
                        result.add_to(rich_text.clone(), &mut self.job_result_cache);

                        // Add result type
                        rich_text.push_color(Color::CORNFLOWER_BLUE);
                        rich_text.add_text(&format!(" ({})", result_type.fancy_type_name()));
                        rich_text.pop();

                        // Add result time
                        rich_text.push_color(Color::CYAN);
                        match (async_wait_time, signal_wait_time) {
                            (None, None) => {
                                rich_text.add_text(&format!(" (took {})", Ms(*execution_time)));
                            }
                            (None, Some(signal_wait_time)) => {
                                rich_text.add_text(&format!(
                                    " (took {} + {} signal wait time)",
                                    Ms(*execution_time),
                                    Ms(*signal_wait_time)
                                ));
                            }
                            (Some(async_wait_time), None) => {
                                rich_text.add_text(&format!(
                                    " (took {} + {} async wait time)",
                                    Ms(*execution_time),
                                    Ms(*async_wait_time)
                                ));
                            }
                            (Some(async_wait_time), Some(signal_wait_time)) => {
                                rich_text.add_text(&format!(
                                    " (took {} + {} async wait time + {} signal wait time)",
                                    Ms(*execution_time),
                                    Ms(*async_wait_time),
                                    Ms(*signal_wait_time)
                                ));
                            }
                        }

                        rich_text.pop();
                    }
                    Err(err) => {
                        tracing::warn!(?err, "expression evaluation failed");

                        self.maybe_add_timestamp();

                        // If help requested, do not print red error text
                        match err.deref() {
                            JobError::EvalClap(EvalClapError::ClapHelp(_)) => {
                                rich_text.push_color(Color::GREEN);
                            }
                            _ => {
                                rich_text.push_color(Color::RED);
                                rich_text.push_bold();
                                rich_text.add_text("Error: ");
                                rich_text.pop();
                            }
                        }

                        // For the different error formats, see https://docs.rs/anyhow/latest/anyhow/struct.Error.html#display-representations

                        rich_text.add_text(&err.to_string()); // basic error (same as {}), no underlying causes are printed
                        // rich_text.add_text(&format!("{err:?}")); // error with backtrace and causes (contains ansi colors) -> does not work well the Godot IoError it seems
                        // rich_text.add_text(&format!("{err:#?}")); // same as above
                        // rich_text.add_text(&format!("{err:#}")); // error with causes, single line, no backtrace

                        rich_text.pop();
                    }
                }

                rich_text.newline();

                Some(jr)
            }
        }
    }

    pub(super) fn set_busy_indication(&mut self, busy: bool) {
        let mut mat = self
            .nodes
            .busy_indicator
            .get_material()
            .expect("busy_indicator has no material")
            .cast::<ShaderMaterial>();

        mat.set_shader_parameter("enabled", &busy.to_variant());
    }

    pub(super) fn draw_slots(&self) {
        let mut lbl = self.nodes.autocomplete_label.clone();
        let suggestions = &self.suggestions.slots;

        match suggestions {
            SuggestionSlots::None => {
                lbl.hide();
                return;
            }
            SuggestionSlots::Search(slots, target_slot) => {
                lbl.show();
                lbl.clear();

                for (
                    i,
                    SearchSlot {
                        name,
                        timestamp,
                        success,
                    },
                ) in slots.iter().enumerate()
                {
                    let is_target_slot = Some(i) == *target_slot;
                    if is_target_slot {
                        lbl.push_bold();
                    }

                    lbl.add_text(name); // Avoid parsing bbcode

                    // color date depending on whether command was successful or not
                    lbl.push_color(if *success {
                        COLOR_SUCCESS
                    } else {
                        COLOR_FAILURE
                    });

                    let date_diff = OffsetDateTime::now_utc() - *timestamp;
                    lbl.add_text(&format!(" {}", humanize_duration(date_diff)));

                    lbl.pop(); // pop color

                    if is_target_slot {
                        lbl.pop(); // pop bold
                    }

                    lbl.newline();
                }
            }
            SuggestionSlots::Autocomplete(slots, target_slot, partial_line) => {
                lbl.show();
                lbl.clear();

                for (
                    i,
                    AutocompleteSlot {
                        name,
                        indentation,
                        help,
                        typ,
                    },
                ) in slots.iter().enumerate()
                {
                    // This string contains everything the user typed before the autocomplete kicked in.
                    // It takes the string `partial_line` and makes it exactly `indentation` characters long, padding/truncating if needed.
                    let prefix_str = format!("{:<1$.1$}", partial_line, indentation);

                    // Add prefix
                    lbl.push_color(COLOR_PREFIX);
                    lbl.add_text(&prefix_str);
                    lbl.pop(); // pop color

                    let is_target_slot = Some(i) == *target_slot;
                    if is_target_slot {
                        lbl.push_bold();
                    }

                    // Add name
                    lbl.push_color(match typ {
                        AutocompleteType::Method(_, _) => COLOR_METHOD,
                        AutocompleteType::UtilityFunc(_, _) => COLOR_UTILITY_FUNC,
                        AutocompleteType::Property(_) => COLOR_PROP,
                        AutocompleteType::Class { is_global, .. } => {
                            if *is_global {
                                COLOR_GLOBAL
                            } else {
                                COLOR_CLASS
                            }
                        }
                        AutocompleteType::BuiltInType => COLOR_BUILT_IN_TYPE,
                        AutocompleteType::Signal(_, _) => COLOR_SIGNAL,
                        AutocompleteType::Clap => COLOR_METHOD,
                    });
                    lbl.add_text(name); // Avoid parsing bbcode

                    lbl.pop(); // pop color

                    // Add arguments/return type
                    lbl.push_color(COLOR_MISC);
                    match typ {
                        // Methods, utility functions, and signals are all method-like, so they're handled similarly
                        methodlike @ (AutocompleteType::Method(args, return_tp)
                        | AutocompleteType::UtilityFunc(args, return_tp)
                        | AutocompleteType::Signal(args, return_tp)) => {
                            lbl.add_text("(");

                            for (i, (arg_name, arg_tp)) in args.iter().enumerate() {
                                if i > 0 {
                                    lbl.add_text(", ");
                                }

                                //  NOTE - in exported game, arg_name can be empty sometimes.
                                if !arg_name.is_empty() {
                                    lbl.push_color(COLOR_PROP);
                                    lbl.add_text(arg_name);
                                    lbl.pop();
                                    lbl.add_text(": ");
                                }

                                lbl.add_text(&arg_tp.nice_type_name());
                            }

                            lbl.add_text(")");

                            if return_tp.typ != VariantType::NIL {
                                lbl.add_text(" -> ");
                                lbl.add_text(&return_tp.nice_type_name());
                            }

                            if matches!(methodlike, AutocompleteType::Signal(_, _)) {
                                lbl.add_text(" (signal)");
                            }
                        }
                        AutocompleteType::Property(tp) => {
                            lbl.add_text(" ");
                            lbl.add_text(&tp.nice_type_name());
                        }

                        AutocompleteType::Class {
                            is_global,
                            is_abstract,
                        } => {
                            if *is_global {
                                lbl.add_text(" (global)");
                            }
                            if *is_abstract {
                                lbl.add_text(" (abstract)");
                            }
                        }

                        AutocompleteType::Clap | AutocompleteType::BuiltInType => {
                            // do nothing
                        }
                    };

                    // Add help
                    if let Some(help) = help {
                        lbl.push_color(COLOR_PREFIX);
                        lbl.add_text(" ");
                        lbl.add_text(help);
                        lbl.pop(); // pop color
                    }
                    lbl.pop();

                    if is_target_slot {
                        lbl.pop(); // pop bold
                    }

                    lbl.newline();
                }
            }
        };

        /////////////

        // The slots were updated, so now we have to update the height of the label.

        let max_height: f32 = 240.; // TODO maybe make this configurable/draggable

        // NOTE - this will only work properly if the label is not threaded...
        // ...unless you make this method async and wait for `finished` signal to know when the doc finished rendering?

        let v_padding = 8; // This corresponds to the total padding below and above the text, you may want to read it from the label directly
        let viewport_height = lbl.get_viewport_rect().size.y;
        let current_y = lbl.get_global_position().y;
        let available_height = viewport_height - current_y; // How high can the label become without going off-screen?

        // Note - get_content_height() is showing up in the profiler.
        // Although that could also just be lazy evaluation kicking in.
        let needed_height = (lbl.get_content_height() + v_padding) as f32;

        // The minimum size of the label becomes the minimum of:
        // 1. how tall we can become without going off-screen
        // 2. the content height, and
        // 3. the maximum total height we want the label to have on screen
        lbl.set_custom_minimum_size(Vector2::new(
            0.,
            available_height.min(needed_height).min(max_height).max(0.),
        ));

        // Scroll to highlighted line
        let target_slot = match suggestions {
            SuggestionSlots::None => 0,
            SuggestionSlots::Search(_, target_slot)
            | SuggestionSlots::Autocomplete(_, target_slot, _) => target_slot.unwrap_or(0),
        };
        scroll_line_into_view(lbl, target_slot as i32);

        // Note - this will not detect the viewport resizing, so if you shrink the window while autocomplete slots are visible,
        // they may go off-screen. It will update once you type a single character again though, so not a huge deal.
    }
}

fn current_local_time_hms() -> String {
    let now = OffsetDateTime::now_utc()
        .to_offset(UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC));

    let format = format_description!("[hour]:[minute]:[second]");
    now.format(&format).unwrap()
}

/// Little helper method that basically replaces `label.scroll_to_line()`.
/// It will instead scroll as little as possible to make the line visible on screen.
/// Note - does NOT work properly if the text in the label wraps around, e.g. try :node find ....
fn scroll_line_into_view(label: Gd<RichTextLabel>, target_line: i32) {
    let Some(mut vscroll) = label.get_v_scroll_bar() else {
        tracing::warn!("RichTextLabel has no scrollbar (scroll_active may be false)");
        return;
    };

    let line_top = label.get_line_offset(target_line);
    let line_bottom = label.get_line_offset(target_line + 1);

    let visible_height = label.get_size().y;
    let current_scroll = vscroll.get_value() as f32;
    let max_value = vscroll.get_max() as f32;

    let viewport_top = current_scroll;
    let viewport_bottom = current_scroll + visible_height;

    let new_scroll = if line_top < viewport_top {
        // Line is above the visible area - scroll up so its top aligns with viewport top
        line_top
    } else if line_bottom > viewport_bottom {
        // Line is below the visible area - scroll down so its bottom aligns with viewport bottom
        line_bottom - visible_height
    } else {
        // Already fully visible - don't scroll at all
        return;
    };

    vscroll.set_value(new_scroll.clamp(0.0, max_value) as f64);
}

// TODO move this to util.rs or something
fn humanize_duration(elapsed: time::Duration) -> String {
    // Let's not pull in a dependency for this
    let secs = elapsed.whole_seconds();

    const SECS_PER_DAY: i64 = 24 * 60 * 60;

    let (value, unit) = if secs < 60 {
        (secs, "s")
    } else if secs < 60 * 60 {
        (secs / 60, "m")
    } else if secs < SECS_PER_DAY {
        (secs / (60 * 60), "h")
    } else if secs < SECS_PER_DAY * 30 {
        (secs / SECS_PER_DAY, "d")
    } else if secs < SECS_PER_DAY * 365 {
        (secs / (SECS_PER_DAY * 30), "mo")
    } else {
        (secs / (SECS_PER_DAY * 365), "y")
    };

    format!("{value}{unit} ago")
}

///////////

pub(super) fn fire_gradient(t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);

    let orange = COLOR_SIGNAL;
    let yellow = Color::from_rgb(1.0, 1.0, 0.0);
    let white = Color::from_rgb(1.0, 1.0, 1.0);

    if t <= 0.5 {
        let local_t = t / 0.5;
        orange.lerp(yellow, local_t as f64)
    } else {
        let local_t = (t - 0.5) / 0.5;
        yellow.lerp(white, local_t as f64)
    }
}

////////////////////////////////////////

/// Uses WATCH_ERROR_DEDUPLICATOR to ensure every unique watch error is printed only once per 5 seconds at most.
/// Returns true if we are allowed to print, else false (rate limited).
///
/// Note - this is on a per-thread basis, every thread gets its own cache.
fn deduplicate_error(err: &JobError) -> bool {
    thread_local! {
        static WATCH_ERROR_DEDUPLICATOR: BoxedCache<String, ()> = {
            RefCell::new(Box::new(
                Cache::builder()
                    .time_to_live(Duration::from_secs(5)) // Use TTL instead of TTI, otherwise it will live too long
                    .build(),
            ))
        };
    }

    let key = format!("{err:?}");

    WATCH_ERROR_DEDUPLICATOR.with_borrow_mut(|c| {
        if c.contains_key(&key) {
            return false;
        }

        c.insert(key, ());
        true
    })
}
