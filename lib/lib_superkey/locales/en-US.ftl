# The superkey — English (source locale).
#
# Every key here must exist in the other three bundles too; a test in
# src/i18n.rs fails if one is missing. Keys start with `superkey-` because the
# bundles are appended to sicompass-ui's, and a clash is a registration error.

superkey-title = Superkey

superkey-section-windows = Windows
superkey-section-controls = Controls
superkey-section-settings = Settings

superkey-no-windows = No open windows
superkey-untitled-window = Untitled window

superkey-button-suspend = Suspend
superkey-button-reboot = Restart
superkey-button-poweroff = Shut down
superkey-button-logout = Log out

superkey-announce-suspend = Suspending
superkey-announce-reboot = Restarting
superkey-announce-poweroff = Shutting down
superkey-power-failed = { $action } failed: { $error }
superkey-launch-failed = Could not start { $name }

# Settings. The wording matches the app's own settings page.
superkey-setting-screen-reader = screen reader
superkey-setting-font-scale = font scale
superkey-setting-color-scheme = color scheme
superkey-setting-language = language
superkey-setting-shoulder-surfing = shoulder-surfing protection (blank screen)
superkey-setting-changed = { $setting }: { $value }
superkey-setting-failed = Could not save { $setting }: { $error }

superkey-on = on
superkey-off = off
superkey-color-dark = dark
superkey-color-light = light

# Each language in its own form, whatever the active locale, so a user can find
# theirs on a screen they cannot read.
superkey-language-en-US = English
superkey-language-nl-BE = Nederlands (België)
superkey-language-fr-BE = Français (Belgique)
superkey-language-de-BE = Deutsch (Belgien)

# Settings is two groups: the session's accessibility, then the bar.
superkey-group-accessibility = Accessibility
superkey-group-bar = Bar
superkey-setting-bar-position = bar position
superkey-setting-bar-seconds = show seconds
superkey-setting-bar-keystrokes = show key strokes
superkey-bar-bottom = bottom
superkey-bar-top = top

# The Status section: what the bar's icons show, in words.
superkey-section-status = Status
superkey-section-tutorial = Tutorial
superkey-section-store = Store
superkey-status-notifications = Notifications ({ $count })
superkey-status-tray = Tray
superkey-no-notifications = No notifications
superkey-dismiss-all = Dismiss all
superkey-notification-dismissed = Dismissed
superkey-tray-empty = Nothing in the tray
superkey-status-failed = That did not work: { $error }
superkey-network-wired = Network: wired
superkey-network-wireless = Network: wireless
superkey-network-wireless-signal = Network: wireless, signal { $strength }%
superkey-network-other = Network: connected
superkey-network-none = Network: not connected
superkey-network-no-internet = no internet
superkey-volume = Volume: { $percent }%
superkey-volume-muted = Volume: muted
superkey-battery = Battery: { $percent }%
superkey-battery-charging = Battery: { $percent }%, charging
superkey-battery-full = Battery: full
superkey-bluetooth-off = Bluetooth: off
superkey-bluetooth-on = Bluetooth: on
superkey-bluetooth-connected = Bluetooth: on, { $count } connected
