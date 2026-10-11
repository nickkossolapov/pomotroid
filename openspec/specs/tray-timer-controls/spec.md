### Requirement: Fixed-width remaining-time tray display

With Show Remaining Time enabled, the system tray SHALL display Inter Regular
digits on a transparent, fixed-size icon instead of the progress ring. The
bundled Google Fonts Inter variable TTF SHALL use its default regular weight
(400). Its SIL Open Font License and copyright notice SHALL be included in
the repository and available offline in Settings → About.

Font source: https://github.com/google/fonts/tree/main/ofl/inter.
Font and license: `src/lib/assets/fonts/Inter.ttf` and `Inter-OFL.txt`.

#### Scenario: Minutes change to seconds

- **WHEN** at least 60 seconds remain
- **THEN** the icon SHALL display rounded-up minutes with at least two digits
- **WHEN** fewer than 60 seconds remain
- **THEN** it SHALL display two-digit seconds without a unit suffix
- **AND** examples SHALL include `1500 → 25`, `1499 → 25`, `60 → 01`, `59 → 59`, `9 → 09`
- **AND** all supported timer durations (up to 90 minutes) SHALL retain identical digit cells, baseline, font size, and icon dimensions

#### Scenario: Exact countdown tooltip

- **WHEN** the tray host supports tooltips and the user hovers over the icon
- **THEN** the tooltip SHALL show the exact remaining `MM:SS` countdown
- **AND** tooltip updates SHALL occur every tick even when the displayed minutes do not change

#### Scenario: Timer state and settings changes

- **WHEN** the tray is created or re-enabled, including during a running or paused round
- **THEN** it SHALL display the current timer rather than a zero or hard-coded work round
- **AND** pause/resume, reset, round changes, appearance changes, and idle duration changes SHALL refresh it
- **AND** changing configured durations during a running or paused round SHALL NOT alter that round's displayed remaining time
- **AND** numeric updates SHALL NOT be throttled by percentage progress
- **AND** paused digits SHALL remain readable with a small indicator below them

#### Scenario: System appearance changes

- **WHEN** the Windows taskbar is dark or light
- **THEN** digits SHALL be white or black respectively, independently of the app theme
- **AND** taskbar theme changes SHALL be observed even while paused
- **AND** macOS SHALL use native template rendering so the menu bar chooses the foreground color
- **AND** other platforms SHALL follow the reported system theme

### Requirement: Automatic window visibility during work

When Hide During Work is enabled and the tray icon is available, the main
window SHALL hide when work starts or resumes, including automatic starts,
tray controls, and keyboard shortcuts. Hiding SHALL use tray visibility rather
than taskbar minimization. Restoration SHALL show, unminimize, and focus the
window without changing its size, maximized state, or fullscreen mode.

#### Scenario: Work stops running

- **WHEN** work pauses, is suspended, completes, is skipped, or is reset
- **THEN** a window hidden by this feature SHALL be restored
- **EXCEPT** when completion or skipping immediately auto-starts another work round
- **AND** starting or resuming a break SHALL NOT hide the window

#### Scenario: Consecutive work rounds

- **WHEN** disabled breaks cause a work round to advance directly to another work round
- **AND** Auto-start Work is enabled
- **THEN** the window SHALL remain hidden without a restore/hide flash
- **WHEN** Auto-start Work is disabled
- **THEN** the window SHALL be restored while waiting for the next start

#### Scenario: Manual restoration during work

- **WHEN** the user restores the window using the tray during running work
- **THEN** subsequent timer ticks and unrelated settings changes SHALL NOT hide it again
- **AND** the next work start or resume SHALL hide it again

#### Scenario: Tray unavailable

- **WHEN** the tray is unavailable, not yet created, or hidden
- **THEN** work SHALL continue without automatically hiding the window
- **AND** the failure to auto-hide SHALL be logged
- **AND** failed window actions SHALL be logged without discarding pending restoration state

### Requirement: Tray menu exposes timer control actions

The tray context menu SHALL include three timer control items — Toggle (Start/Pause/Resume), Skip, and Reset Round — positioned above the existing Show and Exit items.

#### Scenario: Menu items are present when tray is created

- **WHEN** the tray icon is created
- **THEN** the menu SHALL contain items with ids "toggle", "skip", and "reset-round"

#### Scenario: Timer control items appear above Show and Exit

- **WHEN** the user opens the tray context menu
- **THEN** the toggle, skip, and reset-round items SHALL appear before the separator, Show, and Exit items

### Requirement: Toggle item label reflects current timer state

The toggle item SHALL display "Start" when the timer is idle, "Pause" when it is running, and "Resume" when it is paused.

#### Scenario: Label is "Start" on initial tray creation

- **WHEN** the tray icon is first created and the timer is idle
- **THEN** the toggle item label SHALL be "Start"

#### Scenario: Label changes to "Pause" when timer starts

- **WHEN** the timer transitions from idle or paused to running
- **THEN** the toggle item label SHALL update to "Pause"

#### Scenario: Label changes to "Resume" when timer is paused

- **WHEN** the timer is paused
- **THEN** the toggle item label SHALL update to "Resume"

#### Scenario: Label resets to "Start" when timer is reset

- **WHEN** the timer is reset to idle
- **THEN** the toggle item label SHALL update to "Start"

### Requirement: Skip and Reset Round are disabled when the timer is idle

Skip and Reset Round SHALL be enabled only when the timer is running or paused.

#### Scenario: Skip and Reset Round disabled on initial tray creation

- **WHEN** the tray icon is first created and the timer is idle
- **THEN** the skip and reset-round items SHALL be disabled (non-interactive)

#### Scenario: Skip and Reset Round enabled when timer is running

- **WHEN** the timer transitions to running
- **THEN** the skip and reset-round items SHALL become enabled

#### Scenario: Skip and Reset Round enabled when timer is paused

- **WHEN** the timer is paused
- **THEN** the skip and reset-round items SHALL remain enabled

#### Scenario: Skip and Reset Round disabled after reset

- **WHEN** the timer is reset to idle
- **THEN** the skip and reset-round items SHALL become disabled

### Requirement: Toggle item dispatches the correct timer command

Clicking the toggle item SHALL call `timer.toggle()`, which starts the timer if idle, pauses it if running, or resumes it if paused.

#### Scenario: Toggle starts an idle timer

- **WHEN** the timer is idle and the user clicks the toggle item
- **THEN** the timer SHALL start running

#### Scenario: Toggle pauses a running timer

- **WHEN** the timer is running and the user clicks the toggle item
- **THEN** the timer SHALL pause

#### Scenario: Toggle resumes a paused timer

- **WHEN** the timer is paused and the user clicks the toggle item
- **THEN** the timer SHALL resume running

### Requirement: Skip item advances to the next round

Clicking the skip item SHALL call `timer.skip()`, immediately completing the current round and advancing the sequence.

#### Scenario: Skip advances the round

- **WHEN** the timer is running or paused and the user clicks the skip item
- **THEN** the current round SHALL complete and the sequence SHALL advance to the next round

### Requirement: Reset Round item resets the current round only

Clicking the reset-round item SHALL call `timer.restart_round()`, zeroing elapsed time for the current round without changing the round type or position in the work/break cycle.

#### Scenario: Reset Round zeroes elapsed time

- **WHEN** the timer is running or paused and the user clicks the reset-round item
- **THEN** the current round's elapsed time SHALL be zeroed and the timer SHALL return to idle for the same round

#### Scenario: Reset Round preserves sequence position

- **WHEN** the user clicks the reset-round item during round 2 of a work/break cycle
- **THEN** the round type and work round number SHALL remain unchanged after the reset
