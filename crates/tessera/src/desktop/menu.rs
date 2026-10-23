//! Context-menu commands, inbox XAML hosting and custom appearance.
#[cfg(windows)]
mod appearance;
#[cfg(any(windows, test))]
mod shadow;
#[cfg(windows)]
mod xaml;
#[cfg(windows)]
pub(super) use xaml::{ACTION, Controller, FAILED};

pub(super) const PAUSE: u32 = 2;
pub(super) const QUIT: u32 = 3;
pub(super) const POSITION_LOCK: u32 = 4;

#[derive(Clone, Copy)]
enum Icon {
    Pin,
    Unpin,
    Pause,
    Play,
    Exit,
}

struct Command {
    id: u32,
    label: &'static str,
    icon: Icon,
}

fn commands(locked: bool, paused: bool) -> [Command; 3] {
    [
        Command {
            id: POSITION_LOCK,
            label: if locked {
                "取消固定"
            } else {
                "固定位置"
            },
            icon: if locked { Icon::Unpin } else { Icon::Pin },
        },
        Command {
            id: PAUSE,
            label: if paused {
                "继续监控"
            } else {
                "暂停监控"
            },
            icon: if paused { Icon::Play } else { Icon::Pause },
        },
        Command {
            id: QUIT,
            label: "退出",
            icon: Icon::Exit,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_labels_and_icons_follow_lock_and_pause_without_a_settings_item() {
        for locked in [false, true] {
            for paused in [false, true] {
                let [lock, pause, quit] = commands(locked, paused);
                assert_eq!([lock.id, pause.id, quit.id], [POSITION_LOCK, PAUSE, QUIT]);
                assert_eq!(
                    lock.label,
                    if locked {
                        "取消固定"
                    } else {
                        "固定位置"
                    }
                );
                assert!(matches!(
                    (locked, lock.icon),
                    (true, Icon::Unpin) | (false, Icon::Pin)
                ));
                assert_eq!(
                    pause.label,
                    if paused {
                        "继续监控"
                    } else {
                        "暂停监控"
                    }
                );
                assert!(matches!(
                    (paused, pause.icon),
                    (true, Icon::Play) | (false, Icon::Pause)
                ));
                assert_eq!(quit.label, "退出");
                assert!(matches!(quit.icon, Icon::Exit));
            }
        }
    }
}
