use crate::pane::{FocusDirection, SplitAxis};

use super::CommandRegistry;

pub(super) fn register_pane_commands(reg: &mut CommandRegistry) {
    // --- panes ---

    reg.register(
        "buffer.next",
        "Switch the active pane to the next buffer",
        |ctx| {
            ctx.workspace.cycle_buffer(true);
        },
    );

    reg.register(
        "buffer.previous",
        "Switch the active pane to the previous buffer",
        |ctx| {
            ctx.workspace.cycle_buffer(false);
        },
    );

    reg.register(
        "pane.split-right",
        "Split the active pane vertically",
        |ctx| {
            ctx.workspace.split_active_pane(SplitAxis::Vertical);
        },
    );

    reg.register(
        "pane.split-down",
        "Split the active pane horizontally",
        |ctx| {
            ctx.workspace.split_active_pane(SplitAxis::Horizontal);
        },
    );

    reg.register("pane.close", "Close the active pane", |ctx| {
        ctx.workspace.close_view(ctx.view_id);
    });

    reg.register("pane.focus-next", "Focus the next pane", |ctx| {
        ctx.workspace.focus_next_pane();
    });

    reg.register("pane.focus-previous", "Focus the previous pane", |ctx| {
        ctx.workspace.focus_previous_pane();
    });

    reg.register("pane.focus-right", "Focus the pane to the right", |ctx| {
        ctx.workspace.focus_pane_in_direction(FocusDirection::Right);
    });

    reg.register("pane.focus-down", "Focus the pane below", |ctx| {
        ctx.workspace.focus_pane_in_direction(FocusDirection::Down);
    });

    reg.register("pane.focus-left", "Focus the pane to the left", |ctx| {
        ctx.workspace.focus_pane_in_direction(FocusDirection::Left);
    });

    reg.register("pane.focus-up", "Focus the pane above", |ctx| {
        ctx.workspace.focus_pane_in_direction(FocusDirection::Up);
    });
}
