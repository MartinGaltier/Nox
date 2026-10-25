//! When the live levels meter (B7) may run. The decisions are pure: nothing
//! here creates a tap; the engine in `mod.rs` starts and stops the meter.

use nox_audio::MeterEvent;

/// What the user and the player currently ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Gate {
    /// `settings.music.live_levels`.
    pub enabled: bool,
    /// A track is playing.
    pub playing: bool,
    /// The music page is on screen (`MusicCommand::Visible(true)`).
    pub visible: bool,
    /// Pid of the followed player.
    pub pid: Option<i32>,
}

/// Why the meter stays off although the gate is open.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Block {
    /// Only silence arrived: the permission is probably refused. Lasts for
    /// the session, until the option is switched off and on again.
    pub refused: bool,
    /// The meter could not run on this pid. Lasts until the player changes
    /// or the option is switched again.
    pub failed_pid: Option<i32>,
}

/// The pid to tap now, or `None` for no tap at all.
pub(super) fn target(gate: Gate, block: Block) -> Option<i32> {
    if !gate.enabled || !gate.playing || !gate.visible || block.refused {
        return None;
    }
    gate.pid.filter(|&pid| block.failed_pid != Some(pid))
}

/// The block a meter event leaves behind. Levels and `ProcessesLeft` do not
/// block: the latter is restarted on the next step if the player still plays.
pub(super) fn after_event(block: Block, pid: i32, event: &MeterEvent) -> Block {
    match event {
        MeterEvent::Silent => Block {
            refused: true,
            ..block
        },
        MeterEvent::Unavailable(_) => Block {
            failed_pid: Some(pid),
            ..block
        },
        MeterEvent::Levels(_) | MeterEvent::ProcessesLeft => block,
    }
}

/// Switching the option off and on again lifts every block.
pub(super) fn after_settings(block: Block, was_enabled: bool, now_enabled: bool) -> Block {
    if !was_enabled && now_enabled {
        Block::default()
    } else {
        block
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PID: i32 = 4242;

    fn open() -> Gate {
        Gate {
            enabled: true,
            playing: true,
            visible: true,
            pid: Some(PID),
        }
    }

    #[test]
    fn runs_only_when_option_playing_and_visible() {
        assert_eq!(target(open(), Block::default()), Some(PID));
        let off = Gate {
            enabled: false,
            ..open()
        };
        assert_eq!(target(off, Block::default()), None, "option off");
        let paused = Gate {
            playing: false,
            ..open()
        };
        assert_eq!(target(paused, Block::default()), None, "nothing playing");
        let hidden = Gate {
            visible: false,
            ..open()
        };
        assert_eq!(target(hidden, Block::default()), None, "page hidden");
        let no_player = Gate {
            pid: None,
            ..open()
        };
        assert_eq!(target(no_player, Block::default()), None, "no player pid");
    }

    #[test]
    fn player_change_moves_the_tap_to_the_new_pid() {
        let other = Gate {
            pid: Some(PID + 1),
            ..open()
        };
        assert_eq!(target(other, Block::default()), Some(PID + 1));
    }

    #[test]
    fn silence_blocks_restart_for_the_session() {
        let block = after_event(Block::default(), PID, &MeterEvent::Silent);
        assert!(block.refused);
        assert_eq!(target(open(), block), None, "refused: no relaunch");
        let other = Gate {
            pid: Some(PID + 1),
            ..open()
        };
        assert_eq!(
            target(other, block),
            None,
            "still refused on another player"
        );
    }

    #[test]
    fn switching_the_option_back_on_unblocks_silence() {
        let refused = after_event(Block::default(), PID, &MeterEvent::Silent);
        // Off, then on again: the block is lifted.
        let off = after_settings(refused, true, false);
        assert_eq!(off, refused, "switching off alone keeps the block");
        let on = after_settings(off, false, true);
        assert_eq!(target(open(), on), Some(PID));
    }

    #[test]
    fn unavailable_blocks_only_its_pid() {
        let err = MeterEvent::Unavailable("tap : refused".into());
        let block = after_event(Block::default(), PID, &err);
        assert_eq!(target(open(), block), None, "same pid: no retry loop");
        let other = Gate {
            pid: Some(PID + 1),
            ..open()
        };
        assert_eq!(target(other, block), Some(PID + 1), "new player: retry");
        let on = after_settings(block, false, true);
        assert_eq!(target(open(), on), Some(PID), "option switched: retry");
    }

    #[test]
    fn levels_and_processes_left_do_not_block() {
        let block = after_event(Block::default(), PID, &MeterEvent::Levels([0.5; 7]));
        assert_eq!(block, Block::default());
        let block = after_event(block, PID, &MeterEvent::ProcessesLeft);
        assert_eq!(target(open(), block), Some(PID));
    }
}
