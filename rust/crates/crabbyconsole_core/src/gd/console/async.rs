//! Contains async tasks for the console.

use std::{pin::Pin, sync::Arc, time::Duration};

use crabbyconsole_clap::eval_definition::CLAP_COMMAND_PREFIX;
use crabbyconsole_misc::{
    async_util::{
        FutureKind, MaybeTimeout, TimeoutKind, await_untyped_signal, classify_future, wait_until,
    },
    gd::async_node::AsyncGd,
    profile,
    util::wait_and_retain_latest_message_async,
};
use flume::Sender;
use futures_lite::{StreamExt as _, future::or};
use godot::prelude::*;
use scopeguard::defer;
use tap::Pipe;
use task_local::task_local;
use time::OffsetDateTime;

use crate::gd::console::{
    CrabConsole, HistoryEntry, Job, JobExpression, SuggestionSlots,
    autocomplete::AutocompleteType,
    job::{JobError, JobEvent, JobExpressionInner, JobInner, JobResult, JobResultData},
};

task_local! {
    pub(super) static USE_EVAL_TIMEOUT: bool;
}

impl CrabConsole {
    #[tracing::instrument(skip_all)]
    pub(super) async fn toggle_visibility_task(mut self: AsyncGd<Self>) {
        loop {
            tracing::debug!("waiting for tilde press...");

            let Ok(_) = self
                .gd_mut()
                .signals()
                .console_toggle_requested()
                .to_fallible_future()
                .await
            else {
                tracing::warn!("console was freed, stopping toggle_visibility_task...");
                return;
            };

            let was_visible = self.bind().nodes.canvas_layer.is_visible();
            self.bind_mut().nodes.canvas_layer.set_visible(!was_visible);

            tracing::debug!("console visibility toggled");

            if !was_visible {
                self.bind_mut().nodes.line_edit.grab_focus(); // so we can start typing immediately
            } else {
                // Note - we no longer use a SVC, so this may no longer be needed.
                // When you hide the CanvasLayer, the line_edit fires its `hidden` signal.
                // So, it seems to recognize whenever it gets hidden, so it should be smart enough to release focus on its own.
                self.bind_mut().nodes.line_edit.release_focus();
            }
        }
    }

    #[tracing::instrument(skip_all)]
    pub(super) async fn toggle_autocomplete_visibility_task(self: AsyncGd<Self>) {
        let line_edit = self.gd().bind().nodes.line_edit.clone(); // fast clone
        let mut autocomplete = self.gd().bind().nodes.autocomplete_label.clone(); // fast clone

        // This loops between two states:
        // 1. Show the autocomplete label when the textbox gains focus
        // 2. Hide the autocomplete label when the textbox loses focus

        loop {
            let Ok(_) = line_edit
                .signals()
                .focus_entered()
                .to_fallible_future()
                .await
            else {
                return; // Line edit was freed
            };

            autocomplete.show();

            let Ok(_) = line_edit
                .signals()
                .focus_exited()
                .to_fallible_future()
                .await
            else {
                return; // Line edit was freed
            };

            autocomplete.hide();
        }
    }

    #[tracing::instrument(skip_all)]
    pub(super) async fn line_edit_task(self: AsyncGd<Self>) {
        let this = || self.clone(); // <-- prevents having conflicting borrows on `self`
        let mut line_edit = self.bind().nodes.line_edit.clone(); // fast clone

        let mut handle_history_move_requested = {
            let mut line_edit = line_edit.clone(); // fast clone 
            move |dist| {
                tracing::debug!("line_edit_task move detected");

                let hist_count = this().bind().history.len();
                let new_hist_index = this()
                    .bind()
                    .history_index
                    .saturating_add_signed(dist as isize)
                    .min(hist_count.saturating_sub(1));
                this().bind_mut().history_index = new_hist_index;
                let this = this();
                let new_text = &this.bind().history[new_hist_index].command;
                line_edit.set_text(new_text);
                line_edit.set_caret_column(i32::MAX); // move caret to end

                // TODO - prior art: firefox debug console allows you to press down on the last entry,
                // which means it just clears the textbox again.
                // you could impl that by limiting new_hist_index to hist_count instead of hist_count - 1.
                // then you use get(new_hist_index) and if it's None, you set the text to "".
            }
        };

        let mut handle_autocomplete_perform_requested = move |direction: i64| {
            tracing::debug!("line_edit_task autocomplete perform detected");

            // This will trigger the slots to be regenerated and redrawn later
            this().bind_mut().suggestions.dirty = true;

            fn maybe_shift_slot<T>(slots: &[T], slot_index: &mut Option<usize>, direction: i64) {
                match slot_index {
                    Some(slot_index) => {
                        // shift to next/prev slot, wrapping around
                        *slot_index = ((*slot_index as i64) + direction)
                                .rem_euclid(slots.len().max(1) as i64) // prevent panic if len == 0
                                as usize;
                    }
                    slot_index @ None => *slot_index = Some(0), // if None, initialize to Some(0)
                }
            }

            // First shift the selected slot up or down...
            match &mut this().bind_mut().suggestions.slots {
                SuggestionSlots::None => { /* do nothing */ }
                SuggestionSlots::Search(search_slots, slot_index) => {
                    maybe_shift_slot(search_slots, slot_index, direction);
                }
                SuggestionSlots::Autocomplete(autocomplete_slots, slot_index, _) => {
                    maybe_shift_slot(autocomplete_slots, slot_index, direction);
                }
            }

            // ...then edit the text in `line_edit` depending on the new selected slot
            match &this().bind().suggestions.slots {
                SuggestionSlots::Search(search_slots, slot_index)
                    if let Some(target_slot) = search_slots.get(slot_index.unwrap_or(0)) =>
                {
                    line_edit.set_text(&target_slot.name);
                    line_edit.set_caret_column(i32::MAX); // move caret to end 
                }
                SuggestionSlots::Autocomplete(autocomplete_slots, slot_index, partial_line)
                    if let Some(target_slot) = autocomplete_slots.get(slot_index.unwrap_or(0)) =>
                {
                    // Note - self is bound for the duration of this scope

                    // Not sure if we need this, but should prevent panics due to truncate() cutting between char boundaries.
                    fn char_to_byte_index(s: &str, char_idx: usize) -> usize {
                        s.char_indices()
                            .nth(char_idx)
                            .map(|(byte_idx, _)| byte_idx)
                            .unwrap_or(s.len()) // cursor at/past the end
                    }

                    let is_function = match target_slot.typ {
                        AutocompleteType::Method(_, _) | AutocompleteType::UtilityFunc(_, _) => {
                            true
                        }
                        // Do not use _ => {}, so it will error if you add a new type in the future
                        AutocompleteType::Property(_)
                        | AutocompleteType::Class { .. }
                        | AutocompleteType::BuiltInType
                        | AutocompleteType::Signal(_, _)
                        | AutocompleteType::Clap => false,
                    };
                    let new_text = {
                        tracing::debug!(partial_line);
                        let mut line_edit_text = partial_line.clone();

                        // Cut off everything the user typed after the start index of the target slot
                        line_edit_text
                            .truncate(char_to_byte_index(&line_edit_text, target_slot.indentation));

                        // Replace the rest with the target slot
                        line_edit_text.push_str(&target_slot.name);

                        // If this is a function, push parentheses too
                        if is_function {
                            line_edit_text.push_str("()");
                        }
                        line_edit_text
                    };

                    line_edit.set_text(&new_text);
                    line_edit.set_caret_column(i32::MAX); // move caret to end (you could put it between ( and ) if you're feeling fancy)
                }
                _ => {}
            }
        };

        let handle_history_search_toggle_requested = move || {
            tracing::info!("handle_history_search_toggle_requested");

            let search_mode_enabled = this().bind().get_search_mode_enabled();
            this()
                .bind_mut()
                .set_search_mode_enabled(!search_mode_enabled);
        };

        loop {
            tracing::debug!("line_edit_task looping...");

            // Wait for 3 different event types concurrently
            tokio::select! {
                Ok((dist,)) = this().gd().signals().history_move_requested().to_fallible_future() => {
                    handle_history_move_requested(dist);
                },
                Ok((direction,)) = this().gd()
                    .signals()
                    .autocomplete_perform_requested()
                    .to_fallible_future() => {
                        handle_autocomplete_perform_requested(direction);
                    },
                Ok(()) = this().gd()
                    .signals()
                    .history_search_toggle_requested()
                    .to_fallible_future() => {
                        handle_history_search_toggle_requested();
                    }
               else => {
                    // If we get in this branch, that means at least one of the futures returned Err.
                    // So the console was dropped before the signal was fired, so we can stop the loop.
                    tracing::warn!("CrabConsole dropped - stopping line_edit_task");
                    break;
                }
            }
        }
    }

    #[tracing::instrument(skip_all)]
    pub(super) async fn line_eval_task(mut self: AsyncGd<Self>, job_tx: Sender<Job>) {
        let mut line_edit = self.bind().nodes.line_edit.clone(); // fast clone
        let mut rich_text = self.bind().nodes.console_history.clone();

        'outer: loop {
            // Wait for user to submit non-empty text
            let expression = {
                let mut expression = "".to_owned();

                while expression.is_empty() {
                    let Ok((text,)) = line_edit
                        .signals()
                        .text_submitted()
                        .to_fallible_future()
                        .await
                    else {
                        tracing::warn!("line_edit was freed - stopping line_eval_task");
                        break 'outer;
                    };

                    // Note - do NOT trim whitespace at the end, or autocompletion becomes ambiguous
                    expression = text.to_string().trim_start().to_owned(); // Note: this deletes newlines too
                }

                expression
            };
            let expression: Arc<str> = Arc::from(expression);

            // Clear text box
            line_edit.set_text("");

            // Write history only if expression isn't equal to last entry in history, to avoid duplicates
            let write_history = self
                .bind()
                .history
                .last()
                .map(|entry| entry.command.as_str())
                != Some(&*expression);

            let (reply_tx, reply_rx) = flume::unbounded();

            self.bind_mut().maybe_add_timestamp();

            // Print to output
            rich_text.push_bold();
            rich_text.add_text(&format!("> {expression}")); // ignores bbcode
            rich_text.pop();
            rich_text.newline();

            // Connect the signal before sending the request to ensure we don't miss it.
            let interrupt_signal = self
                .bind_mut()
                .signals()
                .interrupt_requested()
                .to_fallible_future();

            // Send request
            let _ = job_tx.send(Job {
                expression: JobExpression::String(Arc::clone(&expression)),
                reply_tx,
                use_timeout: true,
            });

            // Show a spinning indicator in the UI while we're busy
            self.bind_mut().set_busy_indication(true);
            let mut this = self.clone(); // fast clone
            defer! {
                // Using defer here so the busy indicator always stops, even when panicking.

                // Warning - for some reason the HSeparator is gone at this point if you close the game.
                // Which seems weird, as you'd expect the Rust destructors to be called before the Godot destructors.
                // Maybe move this `if` statement into `set_busy_indication` to prevent coupling?
                if this.bind().nodes.busy_indicator.is_instance_valid() {
                    this.bind_mut().set_busy_indication(false);
                }

            }

            #[derive(Debug, PartialEq, Eq)]
            enum InterruptionKind {
                ByUser,
                Panicked,
                ConsoleFreed,
            }

            // Wait for result (Ok) or interruption (Err), whichever comes first.
            // If both arrive simultaneously, prefers result over interruption.
            let result = or(
                async {
                    //  (note - any text_submitted events that come in while waiting will be ignored)
                    let mut stream = reply_rx.into_stream();
                    while let Some(e) = stream.next().await {
                        let apply_event_result = self.bind_mut().apply_event(e); // <- double borrow here?

                        if let Some(JobResult {
                            result,
                            execution_time,
                            async_wait_time,
                            signal_wait_time,
                        }) = apply_event_result
                        {
                            let success = result.is_ok();

                            return Ok(HistoryEntry {
                                timestamp: OffsetDateTime::now_utc(),
                                command: expression.to_string(), // clones the string
                                success,
                                execution_time,
                                async_wait_time,
                                signal_wait_time,
                            });
                        }
                    }

                    // We should only get in this branch if JobEvent::Done never arrived.
                    // That could happen if e.g. the evaluator panics before it can send the message.
                    Err(InterruptionKind::Panicked) // continue 'outer
                },
                async {
                    let Ok(_) = interrupt_signal.await else {
                        tracing::warn!("CrabConsole was freed - stopping line_eval_task");
                        return Err(InterruptionKind::ConsoleFreed); // break 'outer
                    };

                    Err(InterruptionKind::ByUser) // continue 'outer
                },
            )
            .await;

            let mut should_break = false;

            match result {
                Ok(e) => {
                    if write_history {
                        self.bind_mut().history.push(e.clone());
                        self.bind_mut().append_history_to_disk(e);
                    }
                }
                Err(kind) => {
                    // Print to output if we got interrupted.
                    self.bind_mut().maybe_add_timestamp();
                    rich_text.push_color(Color::RED);
                    rich_text.add_text(match kind {
                        InterruptionKind::ByUser => "Interrupted by user.",
                        InterruptionKind::Panicked => "Panicked, see stdout for details.",
                        InterruptionKind::ConsoleFreed => "Console was freed.",
                    });
                    rich_text.pop();
                    rich_text.newline();

                    if write_history {
                        let entry = HistoryEntry {
                            timestamp: OffsetDateTime::now_utc(),
                            command: expression.to_string(), // Clones the string
                            success: false, // We got interrupted so success = false
                            execution_time: Duration::default(), // maybe measure actual duration here?
                            async_wait_time: None,
                            signal_wait_time: None,
                        };

                        self.bind_mut().history.push(entry.clone());
                        self.bind_mut().append_history_to_disk(entry);
                    }

                    if kind == InterruptionKind::ConsoleFreed {
                        // Break later, since we may still need to update history_index first.
                        should_break = true;
                    }
                }
            }

            // Always update history_index, even if !write_history
            let hist_len = self.bind().history.len();

            // Note this is actually out of bounds (+1),
            // so the next time you press up it subtracts 1 from it to get a good index
            self.bind_mut().history_index = hist_len;

            if should_break {
                break 'outer;
            }
        }
    }

    #[tracing::instrument(skip_all)]
    pub(super) async fn autocomplete_task(mut self: AsyncGd<Self>) {
        let rx = self.bind().autocomplete_chan.1.clone();

        // How many autocompletion slots to display in the UI.
        // Note: 150 is pushing it in terms of performance, this causes a ~30ms lag spike every time you type.
        // To increase performance, you can make enable threaded mode on the RichTextLabel.
        // However, this will increase code complexity,
        // since draw_slots() will then need to become async to handle the delayed computation.
        let max_slots = 150;

        let mut prev_line = String::new();

        loop {
            let mut got_new_line = false;

            // For history search, we select on two futures here:
            // 1. wait_and_retain_latest_message_async
            // 2. wait_until(dirty_slots == true)
            // Then, dirty_slots is set to true once the user pressed Ctrl+R.
            // Otherwise, it will still show stale autocomplete slots, if the user switches to search mode.
            let line = tokio::select! {
                Some(line) = wait_and_retain_latest_message_async(&rx) => { got_new_line = true; line },
                () = wait_until(|| self.bind().suggestions.dirty) => prev_line, // use line from previous iteration of the loop
                else => {
                    // Entering this branch means wait_and_retain_latest_message_async returned None.
                    // That means the channel `rx` was closed.
                    tracing::warn!("autocomplete_chan dropped, stopping autocomplete_task...");
                    break;
                }
            };

            prev_line = line.clone();

            // Depending on the console mode, we gather specific kinds of autocomplete slots and store them here.
            if self.bind().get_search_mode_enabled() {
                let mut slots = self.handle_history_search(&line);

                truncate_slots(&mut slots, max_slots);

                let index = match &self.bind().suggestions.slots {
                    SuggestionSlots::Search(_, i) if !got_new_line => *i, // same slot kind + line, so remember previous index
                    _ => None, // different slot kind or line, so reset to None
                };
                self.bind_mut().suggestions.slots = SuggestionSlots::Search(slots, index);
            } else {
                let mut slots = if let Some(clap) = line.strip_prefix(CLAP_COMMAND_PREFIX) {
                    // If line starts with :, evaluate the rest with clap autocomplete
                    self.handle_autocomplete_clap(clap)
                } else {
                    // Else, use GDScript autocomplete.
                    self.handle_autocomplete_gdscript(&line)
                };

                truncate_slots(&mut slots, max_slots);

                let line_edit_text = self.bind().nodes.line_edit.get_text().to_string();

                let index = match &self.bind().suggestions.slots {
                    SuggestionSlots::Autocomplete(_, i, _) if !got_new_line => *i, // same slot kind + line, so remember previous index
                    _ => None, // different slot kind or line, so reset to None
                };
                self.bind_mut().suggestions.slots =
                    SuggestionSlots::Autocomplete(slots, index, line_edit_text);
            }

            // Since autocomplete_slots has been changed, we need to re-draw the slots.
            self.bind_mut().draw_slots();

            // Finally, set dirty slots to false, so we don't re-update every frame.
            self.bind_mut().suggestions.dirty = false;
        }
    }

    /// Evaluates an expression asynchronously without using a return channel, while using a timeout
    ///
    /// It will default to using a timeout if it wasn't set using `USE_EVAL_TIMEOUT.scope(true, async { ... })`
    #[tracing::instrument(skip_all)]
    pub(super) async fn eval_job_without_channel(
        self: AsyncGd<Self>,
        expression: JobExpressionInner,
    ) -> Result<Variant, Arc<JobError>> {
        let job = JobInner {
            expression,
            reply_tx: flume::unbounded().0, // dummy channel
            use_timeout: USE_EVAL_TIMEOUT.try_get().unwrap_or_else(|_| {
                tracing::warn!("USE_EVAL_TIMEOUT wasn't set - defaulting to true");
                true
            }),
        };

        self.eval_job(job).await
    }

    /// Evaluates a job and sends the result on the job's channel.
    /// It also returns the Variant, which is useful since `eval_job` can only run on the main thread.
    /// So, if you're on the main thread, you get access to the Variant, else you get the String (sent in the job's channel).
    ///
    /// Note - also needs Pin/Box, since it's a recursive async fn.
    /// Jobs can eval other jobs, e.g. :set a :sysinfo
    #[tracing::instrument(skip_all)]
    pub(super) fn eval_job(
        mut self: AsyncGd<Self>,
        job: JobInner,
    ) -> Pin<Box<dyn Future<Output = Result<Variant, Arc<JobError>>>>> {
        Box::pin(async move {
            let reply_tx = job.reply_tx.clone();

            // Evaluate the expression while measuring execution time.
            let timeout = if job.use_timeout {
                MaybeTimeout::from(self.bind().timeout)
            } else {
                MaybeTimeout::NoTimeout
            };

            // Uses Tokio timeout, since we want to timeout based on real time.
            let fut = timeout.run_with_timeout(
                USE_EVAL_TIMEOUT.scope(job.use_timeout, self.clone().eval(job.expression)), // fast clone
            );

            // Determine what kind of future we're dealing with.
            let (execution_time, async_wait_time, eval_result) = {
                let mut initial_busy_time = Duration::default();

                let future_kind = profile!("classify_future", classify_future(fut), |t| {
                    initial_busy_time = t
                });
                match future_kind {
                    FutureKind::Sync(result) => (initial_busy_time, None, result), // no idle time
                    FutureKind::Async(fut) => {
                        // Indicate to user/watch we're waiting for an async call.
                        let _ = reply_tx.send(JobEvent::WaitingAsync);

                        future_timing::timed(fut) // measure idle + busy time of the future
                            .await
                            .into_parts()
                            .pipe(|(timing, result)| {
                                // Note - future_timing does NOT measure the busy time BEFORE the first poll.
                                // See documentation for timing.idle().
                                // So, we need to add it manually here...
                                (
                                    timing.busy() + initial_busy_time,
                                    Some(timing.idle()),
                                    result,
                                )
                            })
                    }
                }
            };

            // Replace the tokio Elapsed error type by TimeoutAsync.
            let mut eval_result = eval_result
                .map_err(|_| JobError::TimeoutAsync(timeout.unwrap_duration())) // safe unwrap
                .flatten();

            let mut signal_wait_time = None;

            // Check if it's a signal - in that case, await it (with timeout)
            let mut signal_timed_out = false;
            if let Ok(eval_result) = eval_result.as_mut()
                && let Ok(signal) = eval_result.try_to::<Signal>()
            {
                // Indicate to user/watch we're waiting for a signal.
                let _ = job.reply_tx.send(JobEvent::WaitingSignal);

                // Wait for the signal while measuring time spent waiting.
                let signal_result = profile!(
                    "await_signal",
                    timeout.run_with_timeout(await_untyped_signal(signal)).await,
                    |time| { signal_wait_time = Some(time) }
                );

                match signal_result {
                    Ok(signal_result) => {
                        // Overwrite the original signal with the result from the signal.
                        *eval_result = VarArray::from(&signal_result[..]).to_variant()
                    }
                    Err(_ /* timed out */) => {
                        signal_timed_out = true;
                    }
                }
            };

            if signal_timed_out {
                eval_result = Err(JobError::TimeoutSignal(timeout.unwrap_duration())); // safe unwrap
            }

            // Need this, otherwise we can't clone the error, since we're now sending it to 2 places (channel + return type).
            let eval_result = eval_result.map_err(Arc::new);

            // Prepare result by stringifying the Variant. (We can't send the Variant itself, since it's not Send)
            let result = JobResult {
                result: JobResultData::prepare_job_result(
                    &eval_result,
                    &mut self.bind_mut().job_result_cache,
                ),
                execution_time,
                async_wait_time,
                signal_wait_time,
            };

            // Finally send the final result.
            let _ = job.reply_tx.send(JobEvent::Done(result));

            eval_result
        })
    }
}

fn truncate_slots<T>(slots: &mut Vec<T>, max_slots: usize) {
    if slots.len() > max_slots {
        tracing::debug!(
            "we got {} autocomplete slots -> truncating to {max_slots}",
            slots.len()
        );
        slots.truncate(max_slots);
    } else {
        tracing::debug!("we got {} autocomplete slots", slots.len());
    }
}
