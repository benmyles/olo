# The Olo Book

How to build things with Olo's kits, written for the agents (and
people) who will: what the kits are, how a program made with them is
put together, what to do and what not to, and how to know it's right.
It is to Olo what the Be Book ([vendor/bebook](../vendor/bebook/README.md))
was to BeOS, and it's organised the same way, by kit.

[HACKING.md](HACKING.md) has the rules everything follows (the
responsiveness rule, fast and light, look and feel, fractional
scaling), what each kit widget does in detail ("The kit"), and every
app's own notes. Read its rules first. This book is how the kits work
and how to work with them.

The code in it is real: `crates/kit/examples/book.rs` is the program
walked through in "A whole program", and it's built with the kit, so
the two can't drift. When this book and the code disagree, the code is
right and the book is to be fixed in the same commit.

Contents:

1. [The idea, in a page](#the-idea-in-a-page)
2. [A whole program](#a-whole-program)
3. [The Application Kit](#the-application-kit-olo-app) (`olo-app`)
4. [The Interface Kit](#the-interface-kit-olo-interface) (`olo-interface`)
5. [The kit](#the-kit-olo-kit) (`olo-kit`)
6. [Best practices](#best-practices)
7. [Mistakes already made](#mistakes-already-made)
8. [Testing and measuring](#testing-and-measuring)
9. [Before you commit](#before-you-commit)
10. [From BeOS: a map](#from-beos-a-map)

## The idea, in a page

BeOS gave every window a thread of its own, made everything a window
did the answer to a message, and kept a second thread for each window
in app_server, which went on drawing and moving the window while the
program's own was busy. That is why BeOS felt the way it did, and it is
how Olo's kits work.

- **A looper** is a thread with a queue. It takes what's posted to it
  one at a time, in order, and sleeps when there's nothing (no polling,
  no ticks). The program has one (what happens to the program as a
  whole), and **every window has its own**, so a window that's working
  never holds another up.
- **A view** is a part of a window with state of its own: a struct,
  the messages it can be told (an enum), and a `body` that says what it
  shows for the state it has now. State is changed in one place only:
  the view's `message`, on its window's looper. Nothing else ever
  touches it, so there is nothing to lock.
- **What a view shows is kept.** `body` returns elements (boxes laid
  out by flexbox, controls, drawings). The window keeps a tree of them
  between frames; when a view says it changed (`cx.invalidate()`), its
  `body` runs again and the tree is brought in line with it: what's the
  same is kept as it is, what looks different is drawn again, what
  could have moved is laid out again. Each drawing is kept as a display
  list. Nothing is rebuilt, laid out or drawn because a frame went by.
- **The display thread** is the program's one connection to app_server
  and where its windows are drawn. It keeps what each window last
  showed (its *stage*: every drawing, where it is, what scrolls) and
  draws frames from that. So scrolling, a springing end and animation
  go on at the display's rate while a window's looper works, and a
  window with nothing new costs nothing at all.
- **Work that can take longer than a frame runs on a worker** and
  answers with a message. A looper never waits for anything.

The whole of it: *messages in, state changed, `invalidate`, elements
out*. If you find yourself wanting a lock, a callback into another
view, a timer that polls, or a frame loop, you're about to write the
bug this design removes; look for the message instead.

Three crates, layered:

| Crate | What it is |
| --- | --- |
| `olo-app` (`crates/app`) | The Application Kit: loopers, handlers, messengers, timers, the worker pool. No drawing. |
| `olo-interface` (`crates/interface`) | The Interface Kit: windows on their loopers, views, elements and layout, drawing, the display thread, text, the Wayland client, the GPU renderer. |
| `olo-kit` (`crates/kit`) | What apps are written with: the Interface Kit re-exported whole, Olo's look, the BeOS controls, menus, alerts, panels, the editor, the agent tree, the layout review, upgrades. |

An app depends on `olo-kit` alone and names everything through it
(`olo_kit::View`, `olo_kit::widgets::Button`).

## A whole program

`crates/kit/examples/book.rs` counts the primes below a number: on a
worker, showing how far it has got, stopping when told. It has one of
everything. Run it in a test session (never on the person's screen):

```
cargo build -p olo-kit --example book
T=/usr/bin/python3\ tools/dev/tsession
$T start bk --scale 2 && $T run bk target/debug/examples/book
$T shot bk /var/tmp/bk.png && $T review bk
```

**State and messages.** The view is a struct; everything that can
happen to it is one enum.

```rust
struct Primes {
    below: Child<TextField>,
    /// The count under way: which it is, how far (0 to 1), and the
    /// flag that stops it.
    counting: Option<(u32, f32, Arc<AtomicBool>)>,
    counts: u32,
    found: Option<[u32; 10]>,
    said: String,
}

#[derive(Clone)]
enum Msg {
    Count,
    Stop,
    Progress(u32, f32),               // from a worker: how far count N is
    Counted(u32, Option<[u32; 10]>),  // from a worker: done (None: stopped)
}
```

**`message` is the only place state changes.** A button, a menu item,
a child view's news and a worker's result all arrive here, one at a
time. When what it shows changed, it says so.

```rust
impl View for Primes {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Count => {
                let Ok(below) = self.below.read(cx).text().trim().parse::<u32>() else { /* say so */ return };
                self.stop();
                let stop = Arc::new(AtomicBool::new(false));
                self.counts += 1;
                let which = self.counts;
                self.counting = Some((which, 0.0, stop.clone()));
                let progress = cx.messenger();
                cx.work(
                    move || count(below, &stop, |done| { progress.send(Msg::Progress(which, done)); }),
                    move |found| Msg::Counted(which, found),
                );
            }
            Msg::Stop => self.stop(),
            Msg::Progress(which, done) => match &mut self.counting {
                Some((now, at, _)) if *now == which => *at = done,
                _ => return,            // an earlier count's: old news
            },
            Msg::Counted(which, found) => { /* keep it if it's this count's */ }
        }
        cx.invalidate();
    }
```

Three things to copy from that:

- `cx.work(work, then)` runs `work` on a worker and sends the view the
  message `then` makes of its result. The looper is free meanwhile.
- Progress is a message too: `cx.messenger()` is a handle any thread
  can send this view's messages through. The worker sends a hundred at
  most (see `count`): what would flood a queue is the sender's to
  gather.
- Stopping is a flag the worker reads, and **every answer says which
  request it answers**. A worker that hadn't heard it was stopped may
  still have a message on its way; the view knows it for old news by
  its number.

**`command` takes what was chosen by name**: a menu bar's item, a tool
bar's button, the command palette, an agent. One name, however it was
chosen.

```rust
    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        match command {
            "count" => cx.send(Msg::Count),
            "stop" => cx.send(Msg::Stop),
            "quit" => olo_kit::quit(),
            _ => return false,
        }
        true
    }
```

**`body` says what it shows**, from the state it has, and nothing
else: no state changed, no work started. Controls are told what to
send when used (`cx.msg(..)`); spacing comes from `metrics`, never a
number of your own.

```rust
    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let counting = self.counting.as_ref().map(|(_, at, _)| *at);
        let menus = vec![Menu::new("File").item("Count", "count").key("k")
            .item("Stop", "stop").enabled(counting.is_some())
            .separator().item("Quit", "quit").key("q")];
        let button = match counting {
            Some(_) => Button::new("Stop").on_click(cx.msg(Msg::Stop)),
            None => Button::new("Count").default(true).on_click(cx.msg(Msg::Count)),
        };
        let chart = cx.canvas(|v: &Primes, g, b| { /* draw v.found */ })
            .key_by(|v: &Primes| style::key(&v.found))
            .h(px(60.0));
        let content = div().flex_1().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET))
            .child(div().flex().gap(px(m::SPACING))
                .child(div().flex_1().child(TextControl::new(cx, &self.below).label("Primes below:")))
                .child(button))
            .child(StatusBar::new(counting.unwrap_or(0.0)).label(if counting.is_some() { "Counting…" } else { "" }))
            .child(chart)
            .child(StringView::new(self.said.clone()).dim(true));
        div().size_full().flex().flex_col().child(MenuBar::new(cx, menus)).child(content)
    }
```

The chart is a drawing of the view's own (`cx.canvas`): handed the
view as it is when the drawing is wanted, and, with `key_by`, drawn
again only when what it depends on changed. While the count runs, the
progress bar draws ten times a second and the chart isn't touched.

**`main`** installs nothing by hand: `olo_kit::run` sets up the look,
the keyboard profile, the agent socket and the crash report.

```rust
fn main() {
    olo_kit::run("olo.book-example", |_| {
        olo_kit::open_window(WindowOptions::new("Primes", 360.0, 232.0), |cx| {
            let below = text_field(cx, |f| f.name("Primes below")
                .with_text(olo_kit::restored_state().unwrap_or_else(|| "10000000".into())));
            cx.subscribe(&below, |_: &mut Primes, event: &TextFieldEvent, cx| {
                if matches!(event, TextFieldEvent::Entered) { cx.send(Msg::Count); }
            });
            Primes { below, counting: None, counts: 0, found: None, said: "Nothing counted yet.".into() }
        });
    });
}
```

The text field is a view the window's view owns (`Child<TextField>`):
the owner reads it (`self.below.read(cx)`) and hears what it emits
(`cx.subscribe`). `save_state` (in the example) is what the window
keeps across an upgrade in place: an app with one window carries that
as it is, and finds it in `restored_state()` when it starts again.

Measured, as everything is: at rest it draws nothing (0 frames, 0.0%
of a core); counting, ten frames a second of about 0.2 ms of its own
work each. The layout review found one thing while it was written (the
chart's bars a device pixel off its frame at 1.5×); see "Testing and
measuring".

## The Application Kit (olo-app)

`crates/app`. BeOS's Application Kit as it makes sense now. An app
rarely names it (the Interface Kit wraps what views need), but it is
what everything stands on, and a daemon with no window can be written
with it alone.

- **`Looper`**: a thread with a queue. `Looper::spawn(name, init)`
  starts one and gives a `LooperHandle` (`post(f)`: run this there,
  `quit()`, `waiting()`); on its own thread a `&mut Looper` has
  `after(delay, f)`, `every(interval, f)`, `cancel(timer)`,
  `defer(f)`, `on_idle(f)`, `set_state`/`state`/`state_mut` (one value
  of each type, the looper's own), and `spawn_local(future)`.
- **`Handler`**: something in a looper that takes messages of one type
  (`type Message`; `fn message_received(&mut self, message, looper)`).
  `add_handler` gives its `HandlerId`.
- **`Messenger<M>`**: how any thread sends a handler its messages:
  `send(m)` (false once it's gone; nothing is lost silently if you
  look), `map(f)` to send it something else's news as its own. It
  never blocks and never fills: queues grow.
- **`Latest<T>`**: for what would flood a queue (pointer motion, a
  resize, progress): `put(looper, value, take)` keeps only the newest
  until the looper takes it; `put_with` merges.
- **`ask()`** gives a `Reply<T>` and an `Answer<T>`: the one who's
  asked sends through the `Reply`; the asker awaits the `Answer` in a
  task or takes it as a message. Only a plain worker thread may
  `wait(timeout)` on one. **A looper never waits.**
- **`pool`**: `pool::run(f)` for a blocking call, `pool::spawn(future)`
  for a future, each a `Task<T>` to await. Its threads start when
  there's work and none is free (one a core at most) and leave after
  ten seconds without any: an idle program keeps none. A view uses
  `cx.work` (below), which is this with the answer sent as its message.

What BeOS taught the hard way is left out on purpose: no `Lock()` (what
a looper owns isn't `Send`, so the compiler keeps other threads off
it), no synchronous reply on a looper's thread (two windows asking each
other was BeOS's classic deadlock), no port that fills and blocks its
sender.

## The Interface Kit (olo-interface)

`crates/interface`, re-exported whole by `olo_kit`. Its lower half
(the Wayland client, text, the scene, the GPU renderer) began as GPUI
(see `vendor/README.md`, "From GPUI"); everything from the window up is
Olo's own.

### The program, its windows and their threads

```rust
olo_kit::run("olo.myapp", |app| {            // the main thread: the app's looper
    olo_kit::open_window(options, |cx| MyView { .. });   // a looper of its own
});
```

- `run(app_id, build)` starts the display thread, runs `build` on the
  app's looper and then that looper until the program ends (when its
  last window closes, unless `stay_resident()`; or `quit()`).
- `open_window(options, build)` opens a window on **a looper of its
  own**: `build` runs there and makes its view. It returns a
  `WindowHandle<V>`, which any thread can use: `update(|v, cx| ..)`,
  `messenger()`, `with(|cx| ..)`, `close()`, `is_open()`. All of them
  post to the window's looper and return at once.
- `open_window_here(cx, options, build)` opens a window **on the same
  looper** as the one `cx` is in, and gives a `LocalWindow<V>`. Use it
  for a window that belongs to another and talks to it all the time
  (an alert, a panel, a find window, an inspector): they share a
  thread, so each can reach the other directly. Windows that stand
  alone (documents, a folder each) get `open_window`.
- `open_or_raise(&mut kept, options, build)` for the one-of-a-kind
  window (About, Settings): raised if it's open.
- What happens to the program as a whole comes to the app's looper:
  `message_handler(app, |message, app| ..)` takes what `proxy()` (a
  `Proxy` any thread can hold) sends: another copy's launch
  (`instance::listen`), a setting changed, a daemon's news. Keep what
  the app as a whole knows there (`app.looper().set_state(..)`), and
  tell windows through their handles.

Three kinds of thread, then, and what may run on each:

| Thread | Runs | Never |
| --- | --- | --- |
| The app's looper (main) | launches, app-wide news, opening windows | anything slow; drawing |
| A window's looper | its views' `message`, `command`, `body`, handlers, painting | anything that can take longer than a frame: no file read, no D-Bus call, no lock that a worker holds while it works |
| The display thread | the kit only: input in, frames out, scrolling | your code, ever |
| Workers (`cx.work`, `pool`) | everything slow | touching a view (send it a message) |

### Views

```rust
pub trait View: Sized + 'static {
    type Message: 'static;
    fn body(&mut self, cx: &mut Cx<Self>) -> Element;
    fn message(&mut self, message: Self::Message, cx: &mut Cx<Self>) {}
    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool { false }
    fn close_requested(&mut self, cx: &mut Cx<Self>) -> bool { true }
    fn window_activated(&mut self, active: bool, cx: &mut Cx<Self>) {}
    fn window_closing(&mut self, cx: &mut Cx<Self>) {}
    fn save_state(&mut self, cx: &mut Cx<Self>) -> Option<String> { None }
    // drops: takes_drops, drop_over, dropped; displays_changed
}
```

- **`body`** is called when the view is first shown and after each
  `cx.invalidate()`, once however often that was called before the
  window is next flushed. It must be a function of the view's state:
  the same state, the same elements. It may be called at any time and
  any number of times; never start work, send a message or change
  what it shows from it. (It takes `&mut self` so it can keep what it
  worked out: a cache, a measured width. That isn't state.)
- **`message`** is BeOS's `MessageReceived`. Change the state, then
  `cx.invalidate()` if what's shown changed. Not otherwise: an
  `invalidate` for nothing is a `body`, a comparison and maybe a
  layout for nothing.
- **`command`** takes a command by name; return whether it was this
  view's. What a menu bar, a context menu, the palette or an agent
  chooses goes to the window's own view (`cx.command(name)`,
  `menu::on_command`); it hands on what a part of it should take (a
  pane's "close", the editor's "find").
- **`close_requested`** is `QuitRequested`: return false to keep the
  window (unsaved changes: ask, then `cx.close_window()` from the
  answer).

`Cx<V>` is what a view is handed with everything it's told. It derefs
to `WindowCx` (the window: its size, scale, title, the pointer,
`open_window_here`, `close_window`, …) and adds the view's own:

| | |
| --- | --- |
| `invalidate()` | what I show changed: `body` again before the next frame |
| `invalidate_at_frame()` | it changes faster than frames are shown (a shell printing): once, for the next frame drawn |
| `send(msg)` | a message to myself, taken once what's running now is done |
| `msg(m)`, `msg_with(f)`, `listener(f)` | handlers for elements (below) |
| `messenger()` | my messages, from any thread |
| `view_ref()` | a `ViewRef<V>`: send me messages from closures, children, another view |
| `work(work, then)` | do it on a worker; `then(result)` is my message |
| `spawn(f)` | a future on this looper, to await work done elsewhere |
| `after(delay, msg)`, `after_do(delay, f)`, `every(interval, f)`, `cancel(timer)` | time |
| `new_view(build)` | a view I own (`Child<T>`) |
| `subscribe(&child, f)`, `emit(event)` | hear a child; tell whoever owns me |
| `canvas(paint)` | a drawing of mine, handed me as I am when it's drawn |
| `focus()`, `is_focused()` | the keyboard |
| `invalidator()`, `doer()` | for a part any app's view may hold (the editor): have me say what I show again, or have something done to me, from any thread |

### Elements and layout

`body` returns an `Element`. `div()` is a box; its style is Tailwind's
vocabulary (the `Styled` trait): `.flex()`, `.flex_col()`, `.flex_1()`,
`.gap(px(..))`, `.p(px(..))`, `.w(px(..))`, `.h_full()`, `.size_full()`,
`.absolute()`, `.relative()`, `.items_center()`, `.justify_between()`,
`.overflow_hidden()`, `.border_1()`, `.bg(..)`, `.text_color(..)`.
Children are added with `.child(..)` and `.children(..)`; a string is
a child (text); every kit control is one (`IntoElement`); a child view
is one (`.child(&self.field)`).

- Layout is flexbox (taffy), in points. It runs only for what could
  have moved.
- **Sizes and spacing come from `olo_kit::metrics`** (`WINDOW_INSET`,
  `SPACING`, `TIGHT`, `BOX_INSET`, `CONTROL_HEIGHT`, …), colours from
  the palette (`theme::palette()`), text through `style`. A literal
  number in a `body` is a finding waiting for the review.
- `.id(..)` names an element among its siblings. Give one to anything
  made in a loop whose order can change, so its state (hover, focus, a
  scroller's place) follows it.
- `uniform_rows(count, row_height, make)` is a list of rows all the
  same height: only the rows near what shows are ever made. The kit's
  `ColumnListView` is built on it; use that for lists.
- `.measured(f)` for an element whose size depends on the room it's
  given (wrapped text of your own).

### Hearing things

Elements take handlers (the `InteractiveElement` trait): `on_click`,
`on_mouse_down(button, ..)`, `on_mouse_up`, `on_mouse_move`,
`on_hover`, `on_scroll_wheel`, `on_key_down`, `capture_key_down`,
`on_text_input`, `on_drop`, `on_focus_changed`, `on_bounds_changed`,
`on_scroll`, and for a drag that goes on outside the element,
`on_window_mouse_move` / `on_window_mouse_up`. A handler is a
`Fn(&Event, &mut WindowCx)`; a view makes them with:

```rust
.on_click(cx.msg(Msg::Save))                                   // send this
.on_toggle(cx.msg_with(|on: &bool| Msg::Wrap(*on)))            // send what f makes of it
.on_mouse_down(MouseButton::Left, cx.listener(|v: &mut Me, e: &MouseDownEvent, cx| { .. }))  // reach me now
```

Prefer `msg` and `msg_with`: everything that happens to the view then
reads as one `match` in `message`. Use `listener` when the answer must
be now (a press that starts a drag).

- `.hover(|s| ..)`, `.active(|s| ..)`, `.focus(|s| ..)` restyle an
  element under the pointer, pressed, or with the keyboard, with no
  handler and no `body`: the window draws that node again by itself.
- `.focusable()`, `.tab_stop()`, `.focus_within()` (a frame that
  counts the keyboard as its own while something inside has it),
  `.default_key()` (Return's).
- `.occlude()` keeps the pointer from what's under an element;
  `.raised()` paints it over its siblings.
- `cx.stop_propagation()` in a handler keeps what's under from hearing
  the same event.
- Where a pointer event is in an element that may have scrolled:
  `cx.listener_bounds()`, the element's place in the window now. Never
  a place you kept from the last time it was drawn.

### Drawing

`canvas(|g, bounds| ..)` is an element that draws: `g` is a `Painter`,
`bounds` where it was laid out, in the window's points.

- `canvas(paint)`: needs nothing of the view (it owns what it draws).
- `cx.canvas(|v: &V, g, b| ..)`: handed the view as it is when the
  drawing is wanted, so it can't draw stale state.
- `control_canvas(|g, b, state| ..)`: also handed whether it's
  hovered, pressed, focused; drawn again when that changes, with no
  handler of yours.

**A drawing is kept.** It's drawn again only when the element is new,
its size changed, the view that made it said what it shows again *and*
its key differs, or it asked to be. So:

- **Give every drawing a key**: `.paint_key(style::key(&(a, b, c)))`
  for a plain `canvas`, `.key_by(|v: &V| style::key(&v.x))` for a
  view's. The key is everything its look depends on, hashed. Without
  one, a drawing is drawn again every time its view's `body` runs;
  with a wrong one (something left out), it goes stale. A key is the
  cheapest speed-up there is and the easiest bug to write.
- A drawing **only looks**. It can't change the view. What it finds
  out that the view needs (where something landed, for hit testing)
  goes in a `Cell`/`Rc<RefCell>` the two share; what it lacks, it
  shows a placeholder for and asks with a message.
- Draw what shows: `g.visible_bounds()` is what can be seen of the
  drawing (it's then drawn again as that changes). A tall drawing in
  a scroll view must not draw what's off screen.
- `g.paint_again_within(seconds)` for what moves with time (a clock's
  hand, a spinner): asked for per drawing, so nothing else runs.
  Never a timer that invalidates the view to animate.
- **Paint in any order that reads well; it's drawn in few draws.** A
  drawing keeps what it paints in *orders*: things of one kind (boxes,
  glyphs, pictures, lines) that aren't over one another share one,
  wherever they were painted, and an order is one draw on the GPU. A
  thing goes above what it's painted over and nothing else, so a list
  painted row by row (ground, icon, name; ground, icon, name) is
  three draws, not three a row. What you paint over something is
  always drawn after it. You needn't sort by kind to be fast, and
  mustn't rely on paint order between things that don't overlap.
  `OLO_FRAME_PHASES=1` says how many draws ("batches") each frame
  was.
- Everything lands on whole device pixels at every scale when it goes
  through `style` (`fill_rect`, `inset`, `hline`, `sunken`, `raised`,
  `label`, `snap`, `snap_centred`) and `icons::paint`. Use them.
  `g.paint_quad`, `paint_path`, `paint_segment(s)`, `paint_sector`,
  `paint_image`, `paint_yuv` are underneath, for what `style` hasn't.
  A picture is never resampled: one texel a device pixel.
- Say what a drawing shows to agents: `agent::record(g,
  agent::Node::button(name, bounds))` (and its kin), or on the element
  `.agent(role, name)`. Kit controls do it themselves.
- A view that scrolls what it draws itself calls
  `g.note_scroll([x, y, max_x, max_y])`, so its scrolling can be
  measured like any other.

### Scrolling

A box with `.overflow_y_scroll()` (the kit's `ScrollView` is the one to
use: it has the bars) scrolls on the display thread: the wheel and the
touchpad move it there, at the display's rate, whatever the looper is
doing, and its content isn't drawn again to move.

- `ScrollHandle` is where it is: `offset()` (zero at the start, down
  to minus `max_offset()`), `scroll_to_item(i)`, `scroll_to_bottom(cx)`,
  `reveal_span(top, bottom)`, `reveal_in_middle(cx, top, bottom)`,
  `set_offset_once_laid_out(..)`. What's asked of it waits for layout
  (the view's height isn't known before); it always lands on whole
  device pixels.
- The looper hears where it got to (`.on_scroll(..)`,
  `.watch_scroll(&handle)` to draw again as it moves: a scroll bar's
  knob does). Don't read `offset()` in a `body` to decide what to
  show unless that element watches the scroller.
- A view that must scroll itself (the editor lays out only the
  paragraphs that show; Terminal has its emulator's scrollback) takes
  the wheel (`on_scroll_wheel`), keeps an `Elastic` for its ends, and
  draws at its own offset. That's the exception; it costs a `body` or
  a paint per step, on the looper.

### Text and typing

- A string child is text in the element's text style (`.text_size`,
  `.font_weight`, `.text_color`, `.truncate()`, `.line_clamp(n)`).
  The kit's `StringView` is a line as tall as a control; use it in
  forms.
- In a drawing: `style::label(g, x, y, align, text, size, font,
  colour)` and its kin (`label_fit`, `label_on_baseline`). They shape
  once and keep the glyphs.
- Text that's cut short is noted (`olo_kit::cut`): the person gets it
  whole in a tip, and agents read it whole.
- Typing goes to the view with the keyboard through `TextInput`; you
  will almost never implement it: use `text_field` (a line or a few)
  and `olo_kit::editor` (a document).

### Time

- `cx.after(delay, msg)`: once. `cx.every(interval, f)`: until the
  view ends or `cx.cancel(timer)`. `cx.after_do(delay, f)`: a closure
  on the view, for what's due at a time worked out as it goes.
- The kit's `Due` is one timer for the next thing a view has due, and
  `Notice` a status message that clears itself. Use them rather than
  an `every` that looks to see whether it's time.
- **Nothing polls.** A timer is for time (a clock's minute, a caret's
  blink, a debounce), never for looking to see whether something
  changed: that has an event (a file: `olo_kit::watch`; a setting:
  `settings`; a child: `subscribe`; a worker: its message).
- `cx.on_next_frame(f)` is for animation the kit itself paces. A
  window that drew nothing gets no frame: work that must go on with
  nothing to draw runs on a timer, not on frames.

### Work

Anything that can take longer than a frame (8 ms at 120 Hz), or whose
time you don't control: files, the network, D-Bus, decoding, a search,
a subprocess's answer.

```rust
cx.work(move || read_the_thing(path), Msg::Read);     // result -> message
```

- Progress: the worker holds `cx.messenger()` and sends messages;
  bound how many (every 1%, or through `Latest`).
- Cancelling: a flag the worker reads (`Arc<AtomicBool>`), set when
  the view no longer wants the answer; and a number in every answer,
  so old news is known when it arrives.
- Streaming results (a search, a folder's listing): send them in
  batches as they come; the view shows what it has. Never collect
  everything and then show it.
- A view that ends drops its messenger's other end: a worker's `send`
  returns false, and that's its cue to stop.
- See "The responsiveness rule" in HACKING.md: live progress, results
  as they come, cancellable, never a frozen window.

### Views in views, windows and what they share

- **`Child<V>`** is a view another owns: made with `cx.new_view(..)`
  (or a kit helper like `text_field(cx, ..)`), kept in the owner's
  state, placed in its `body` (`.child(&self.field)`). It can't be
  copied. **Only its owner reaches into it**: `child.update(cx, |v,
  cx| ..)` runs now, `child.read(cx)` reads it. Since the owner can't
  be inside the child when it does, nothing ever reaches a view that's
  busy, and there's no re-entry to guard against.
- **A child tells its owner** by emitting: `cx.emit(Event)` in the
  child, `cx.subscribe(&child, |me, event, cx| ..)` in the owner. The
  owner's `body` doesn't run because the child's did; if the owner
  shows something of the child's (a title, a count), the child emits
  when that changes.
- **`ViewRef<V>`** is a handle that doesn't own: `send(cx, msg)`, and
  `update(cx, f)`, which runs **later** (once what's running is
  done), and reaches a view in another window of the same looper too.
  From other threads: `view_ref.messenger(cx)`, or `cx.messenger()`
  taken beforehand.
- **Between windows on different loopers**: messages only, through
  `WindowHandle::messenger()`/`update(..)`, or the app's looper
  (`proxy()` and `message_handler`) for what several windows need to
  hear.
- **What several windows show** (the settings Preferences' panels
  edit, Passwords' vault) is one value behind a lock in an `Arc`
  (`olo_sync`'s locks check their order in debug builds), with two
  rules: take what you need and let go *before* calling the kit (a
  handler that holds it while opening a window deadlocks the day that
  window's view takes it); and each window is *told* when it changed
  (a message), never left to look.

### Windows

`WindowOptions::new(title, width, height)` then: `.min_size`,
`.max_size`, `.fits_content()` (as tall as what it shows needs, never
taller than the smallest screen: every form, panel and dialog),
`.dialog()`, `WindowOptions::alert(..)`, `.floating()`, `.child()`,
`.fullscreen()`, `.keep_size(name)`, `.restore_key(key)`,
`.layer(namespace, layer, anchors)` for a layer surface (Deskbar, the
desktop, notifications), `.no_panel()`.

- A window whose content sets its size uses `fits_content()`; never
  work out a window's height by hand, and never wrap a whole window's
  content in a scroll view of its own. What doesn't fit 1280 × 800 is
  laid out again (side by side, tabs), not scrolled. (HACKING,
  "Windows fit what they show".)
- Menus over a window: `menu::context_menu(cx, menu, at, on_choose)`,
  `menu::popup(cx, menu, anchor, corner, on_choose)`. Alerts:
  `widgets::open_alert(cx, title, icon, message, buttons, on_choose)`.
  Files: `file_panel::show(title, FilePanel::open(dir).types(..),
  |answer| ..)`. All come back as a call to your handler (make it
  with `cx.msg_with`).
- A window the app must outlive (an error about a file that wouldn't
  open) is opened *before* the one it's about is closed, or the app
  ends between them.

### Frames: what is drawn when

What a window is told is taken one at a time; when its queue is dry it
is **flushed**: invalidated views' bodies run, the tree is brought in
line, what could have moved is laid out, what changed is painted, and
the lot is **committed** to the stage on the display thread, which
draws a frame. A burst of messages is one flush.

- No message, no flush; no commit, no frame. An idle window's threads
  sleep. Check it: `tools/dev/cost PID` on a window at rest must say
  0 frames and 0.0% of a core.
- The stage draws a frame for a commit, for a scroller that moved, for
  an end springing back, and for a drawing that asked
  (`paint_again_within`), and for nothing else. It draws only the
  parts of the window that changed (a few rectangles apart: a line
  typed in and the status line under it), and of a drawing made again
  in the same place only what differs from the last.
- `invalidate_at_frame()` holds a view's change for the next frame
  the compositor asks for, so a flood (a shell printing megabytes) is
  laid out and painted once a frame, as it is then.
- The compositor sends input and then calls for a frame. What a
  looper makes of that input comes back half a millisecond later, so
  a frame the stage would draw at the call waits for it (four
  milliseconds at most) and both are drawn at once: a view that
  scrolls itself stays in step with the fingers while a worker's
  results arrive around it.
- Every frame is marked (`{"frames":true}` on the app's socket): when
  it was drawn, the looper's work, the whole, and where each scroller
  was. `cost`, `bench` and `scroll-check` read those. A mark's `ui` is
  the looper's work for the window, not time spent waiting (for a
  frame, for the compositor, for a new window to be made).

## The kit (olo-kit)

`crates/kit`: what every app is written with. HACKING.md's "The kit"
describes each widget in full; this is the map.

**The look.** `theme::palette()` is the palette in force (light, dark,
night: never a colour of your own); `metrics` the spacing; `style` the
BeOS drawing primitives on device pixels (`sunken`, `raised`, `bevel`,
`fill_rect`, `hline`, `label`, `key`, `snap`); `icons::paint(g,
bounds, name)` an icon at one texel a device pixel.

**Controls** (`olo_kit::widgets`), each a builder that's an element:

| BeOS | Olo | Notes |
| --- | --- | --- |
| `BButton` | `Button::new("OK").default(true).on_click(..)` | `.small()`, `.enabled(false)`, `.pushed(..)` |
| `BCheckBox`, `BRadioButton` | `CheckBox::new(label, on).on_toggle(..)`, `RadioButton::new(label, on).on_select(..)` | `.mixed(true)` |
| `BTextControl` | `TextControl::new(cx, &field).label("Name:")` | the field is a `Child<TextField>` from `text_field(cx, ..)` |
| `BTextView` | `text_view`, `olo_kit::editor` | the editor: documents, undo, find, spelling, source code |
| `BMenuField` | `MenuField::new(id, items, chosen).label(..).on_change(..)` | `MenuField::many(id, items, chosen, none)` for several choices, not a row of check boxes |
| the Installer's "More Options" latch | `PaneSwitch::new("More Options", open).id("more").on_toggle(..)` | you keep `open` and put the pane under it; the window follows its content |
| `BSlider` | `Slider::new(id, value, range).on_change(..)` | `.step`, `.limits`, `.value_text` |
| `BStatusBar` | `StatusBar::new(fraction).label(..).trailing(..)` | |
| `BStringView` | `StringView::new(text)` | `.dim(true)`, `.wrap(true)`, `.cut(true)` |
| `BBox` | `BBox::new("Title").child(..)` | `.action("Reset", ..)` |
| `BScrollView`, `BScrollBar` | `ScrollView::new(&handle).child(..)` | `.horizontal(true)`, `.auto_hide(true)` |
| `BListView`, `BColumnListView` | `ColumnListView::new(cx, id, &model, columns, rows, cell)` | virtual; sort, type-select, drag, context menus |
| `BTabView` | `TabView::new(id, tabs, chosen).on_select(..).child(..)` | Terminal's strip is `TabStrip` |
| `BMenuBar`, `BMenu`, `BPopUpMenu` | `MenuBar::new(cx, menus)`, `Menu::new("File").item(label, command).key("s")`, `menu::context_menu` | commands arrive in `View::command` |
| `BAlert` | `open_alert(cx, title, icon, message, buttons, on_choose)` | the last button is Return's, the first Escape's |
| `BFilePanel` | `file_panel::show(..)` | open, save, a folder |
| `BColorControl` | `ColorControl::new(id, rgb).on_change(..).build(cx)` | |
| | `Sidebar`, `SidePane`, `ToolBar`, `Banner`, `KeyHint`, `Graph`, `Spectrum`, `CellGrid`, `ThumbnailPicker`, `find_bar` | what BeOS didn't have |

**If an app needs a widget or behaviour another could use, it goes in
the kit** (`crates/kit`), documented in HACKING.md, with an example in
`crates/kit/examples` and, if it lays things out, a lint test.

**What every app does the same way** (HACKING.md, "What every app
does the same way"): one process an app (`instance::forward` first in
`main`, `instance::listen` in `run`), menus with the standard items
and keys from the keyboard profile, undo (`olo_kit::undo`), the
clipboard, drag and drop (`olo_kit::dnd`), Open in Tracker, recent
documents, window places kept, search and filtering
(`olo_kit::search`).

**Agents see what people see.** Every kit app answers on a socket
(`olo-menus-PID.sock`): its menus (`{"menus":true}`, and
`{"invoke":"command"}` to choose one), each window's tree of what it
shows (`{"tree":true}`: role, name, value, where; kit controls record
themselves), its layout review (`{"review":true}`) and its frames
(`{"frames":true}`). `olo-screen tree`/`click --role Button --name OK`
drive apps through it. A custom drawing that a person can read or
press must record what it shows, or agents are blind to it and the
lint can't check it.

**Surviving an upgrade.** Apps re-execute in place when a new build is
installed and reconnect when app_server restarts. Each window's own
view says what it keeps (`View::save_state`); an app with several
windows makes one state of them (`on_upgrade(|windows| ..)`: handed
what each kept); `restored_state()` hands it back as the program
starts. Windows come
back where they were by themselves; what they showed (which document,
which tab, what was typed) is yours to keep. Test it: `tsession start
NAME --session`, rebuild, `tsession reload NAME`.

## Best practices

Each of these is here because breaking it cost a bug or a slow frame.

**State and messages**

1. Everything that can happen to a view is a variant of its `Message`.
   If you're passing closures that mutate the view around, make it a
   message.
2. Change state only in `message` (and `command`, and handlers made
   with `cx.listener`). Never in `body`. Never from a drawing.
3. `invalidate()` when what's shown changed, and only then.
4. Every answer from a worker says which request it answers. Assume
   the answer to a cancelled request will still arrive.
5. A view doesn't know who owns it: it emits, and the owner
   subscribes. A child that calls up into its parent has the
   dependency backwards.
6. Shared between windows: behind a lock in an `Arc`, let go before
   any call into the kit, and every window told when it changes.

**Never block, never poll**

7. No file, socket, D-Bus call, decode or subprocess on a looper.
   `cx.work`.
8. No `sleep`, no waiting on a channel, no `Answer::wait` on a looper.
9. No timer that looks for changes. Find the event.
10. Show progress, stream results, let it be stopped.

**Drawing**

11. Every `canvas` has a key with everything its look depends on.
12. Draw only what `visible_bounds()` shows.
13. Go through `style` and `icons::paint`: device pixels at 1×, 1.25×,
    1.5×, 1.75× and 2×. Colours from the palette, spacing from
    `metrics`.
14. Animation is `paint_again_within`, per drawing. A window at rest
    draws nothing.
15. Whatever is drawn by hand and can be read or pressed is recorded
    for agents.

**Layout**

16. Let layout do it. A window that works out its children's
    positions by hand breaks at the first scale or font change.
17. `fits_content()` for anything whose content sets its size.
18. Lists are `ColumnListView` (or `uniform_rows`): never one element
    a row for a thousand rows.

**The kit**

19. Before writing a control, look in `olo_kit::widgets` and
    `crates/kit/examples`. Before writing a behaviour, look in
    HACKING.md's "What every app does the same way".
20. Before designing anything, look at how BeOS did it
    ([the Be Book](../vendor/bebook/README.md), Tracker, the kits);
    keep what still makes sense, and use a better modern way where
    there is one.

## Mistakes already made

Found moving 42 apps onto the kits. Each is a rule above seen from the
other side.

- **A drawing read state the view had since changed**, because it
  captured a copy when `body` ran and was kept. Use `cx.canvas` (handed
  the view as it's drawn) and a key.
- **A drawing never drawn again**, because its key left something out
  (the theme, a hover, the scale). The key is *everything*.
- **A pointer handler used a place kept from the last paint**: wrong
  after a scroll, and nothing at all in a window just opened (nothing
  had moved over it). A drawing's place is the bounds it's handed; a
  pointer's is `cx.listener_bounds()`.
- **Work on `on_next_frame` that stopped**, because a window that
  draws nothing gets no frames. Timers.
- **A parent showing a child's title that never updated**: the
  parent's `body` doesn't run with the child's. The child emits.
- **`ViewRef::update` taken for immediate**: it runs later.
  `Child::update` runs now.
- **A scroll asked for before layout** landed at the top: the view
  had no height yet. `set_offset_once_laid_out`, `reveal_in_middle`.
- **A lock held across a call into the kit** deadlocked when the
  window that call opened took it. Take, copy out, let go, then call.
- **An app ended between two windows**: the error alert opened after
  the document's window closed, and the last window closing ends the
  app. Open the next before closing the last.
- **A list on a tab that wasn't showing kept its scroll offset across
  a scale change** and came back a third of a point off, every row
  between pixels. (Fixed in the kit: scroll offsets are snapped when a
  node takes its scroller. The lesson is the test: change the scale
  *while the app runs*, with every tab visited.)
- **"The same to the pixel" was the same failure twice**: both shots
  were of a leftover overlay, or both were blank. Look at one picture
  of every batch.
- **A measurement taken beside a build**: the same scroll read 0.95 to
  1.04 of the fingers' speed alone, and 0.00 to 2.20 with cargo on
  every core. Look at `uptime` first. **And one taken straight after
  a build**: live resizes cost ten times the processor (buffers are
  slow to come by while memory is full of what the build wrote), which
  looked like a regression in the kit for an hour. A result that
  surprises is run again, alone, before anything is made of it.
- **Two builds compared by their own marks**: every menu in every app
  "took 9 to 17 ms" on the kits against 1 on GPUI, because the kits'
  marks counted the window's making and GPUI's didn't. From the press
  to the menu's picture, measured the same way for both, it was 4 ms
  in each.
- **A fix that drew twice**: to keep a scroll in step, the answer to
  input was drawn at once even when a frame had just been drawn. No
  stall, and forty frames too many in one drag. The fix that stayed
  holds the first frame for the answer instead. Count the frames a
  fix draws, not only the ones it stops skipping.
- **And that fix, checked on what it was for and nothing else**: the
  hold was tried on a drag and a flick, which it mended, and committed
  to a whole benchmark only afterwards, which found every wheel notch
  in the editor and Terminal drawn as one jump, 180 ms late: the
  wheel's ease made its steps inside the frame call, each step started
  a hold, and each hold's end asked for a frame. A change to how
  frames are paced is checked against every way a frame comes about
  (fingers, a glide, the wheel, typing, a resize), before anything
  else is built on it.
- **A structure handed about by value that was 3 KB**: a shaped line
  had room in it for 32 decoration runs, and every label painted
  copied it twice. Nothing in a frame's phases said so; `perf` said
  "libc", and `tools/dev/memhist` said whose. Before keeping a list
  inside a structure (`SmallVec<[T; N]>`), multiply: N times the
  size of T is moved every time the structure is. And an allocation
  in something called for every label (a font's empty feature list,
  a string copied to make a key for a lookup) is thousands a frame.
- **A frame's cost over-counted**, because marks said when a frame's
  cause began, not when it was drawn. (Fixed in the kit.) The lesson:
  when a number looks too good or too bad, read how it's made before
  believing it.
- **A shortcut that did something else**, because its menu item was
  off when it shouldn't have been: Terminal's Copy went by the
  selection as it was when the menus were last said, and selecting
  didn't say them again, so ⌘C went to the shell as Ctrl+C. A window's
  menus are as its last body left them: when what an item's `enabled`
  reads changes, ask for the window's body (`Cx.redraw`), whatever
  else that change draws. Test a shortcut by pressing it, with the
  state it needs made the way a person makes it (a double-click, not a
  call), not by choosing its item.
- **Work done for a result nothing used.** Every small shape (a
  menu's triangle, a check mark, a row's arrow) was cut into
  triangles as it was built, a dozen allocations and six kilobytes
  each, and then drawn from a cached mask that never looked at them:
  half of all a Tracker window allocated as it was resized. Found by
  counting allocations by who asked (`tools/dev/memhist`), not by the
  profile, which said "malloc". When something is made on the way to a
  drawing, check that the drawing reads it. And the same window told
  its app its size at every step of a resize, for the app to work out
  what it watches again: say to another looper what it acts on, and
  keep the rest where it can find it.
- **A key read as it is "now"**, by a thread that takes it later. The
  display thread hears the keys; a window's looper handles them when
  it gets to them. Whether ⌘ was held was a global the looper read at
  that moment: ⌘C let go quickly, or taken by a busy window, was
  Ctrl+C. What an event needs to be understood travels with the event
  (its modifiers, its time, its number); nothing about "then" is asked
  of "now".
- **Permission decided by an inherited variable.** Whether a program
  may ask for the administrator went by a variable handed down from
  olo-session, which a program started any other way (by app_server
  before the variable was set, by systemd, by D-Bus) didn't have: in
  the person's own session, "not in the person's session". Decide by
  what is so (which display it's on, as the session itself says), and
  when a check refuses, have it say what it went by.
- **A service that was running, useless, and reported as not
  running.** Three mistakes in a row. Something was set by hand to get
  going (a driver loaded with an option) and nothing kept it, so a
  restart undid it. The service ended when the driver refused it,
  instead of saying so and going on. And its reader took "has said
  nothing" for "isn't running". Whatever a thing needs to work, the
  thing itself keeps (or its installer does, every time, not "where
  it's missing now"); a service says what it can't do and stays; and
  "is it running" is asked of what knows (its folder in /run, systemd),
  never inferred from a file it may not have written yet.
- **A test that pressed Return on a list** opened a real browser in
  the test session. Know what a key does to what has the keyboard
  before sending it; tests act on made-up data, fixture homes and
  stand-ins only (HACKING, "Testing").

## Testing and measuring

All of it headless, in sessions of your own (`tools/dev/tsession`,
run with `/usr/bin/python3`), never on the person's display, and never
by starting an app's binary from your shell (even `--help` opens it on
their screen).

**Does it look right?**

- `tsession start NAME --scale 2`, `tsession run NAME target/debug/…`,
  `tsession shot NAME out.png`, `tsession zoom …`. Then `tsession
  scale NAME 1`, `1.5`, `1.25`, `1.75` *while it runs*, and `tsession
  theme NAME dark`. It must be perfect at every one.
- `tsession review NAME` after each: the layout review reads what was
  actually drawn (edges between device pixels, seams, things over each
  other, text cut, uneven spacing, a window that doesn't fit what it
  shows). A finding at one scale only is a real bug until shown
  otherwise. While the book's example was written it found the chart's
  bars one device pixel off their frame at 1.5× and nowhere else.
- `cargo ui-lint`: the kit's forms and every Preferences panel through
  the same review with no display, at 1×, 1.5× and 2×, light and dark
  (the pre-commit hook runs it). Add a test for a new widget
  (`crates/kit/tests/lint.rs`: `Lint::new().window(&options, |cx|
  View { .. })`, then `assert_clean`).
- `tools/dev/ui-lint`: the same of every app, live, each in a session
  of its own (the pre-push hook runs it).
- Changing something global (a metric, hinting, how a control draws)?
  `tools/dev/kit-compare OLD NEW`: two builds of the same window,
  pixel for pixel, at every scale and theme, to show it changed what
  you meant and nothing else. Keep the old build with `cp
  --reflink=always` under `/var/tmp`.

**Does it behave right?**

- Drive it as an agent would: the tree (`{"tree":true}` on its
  socket, or `olo-screen tree`), then `tsession click`, `key`, `type`,
  `scroll`. Check what the tree says afterwards, not only the picture.
- Reload in place: `tsession start NAME --session`, rebuild, `tsession
  reload NAME`: its windows come back as they were.

**Is it fast?** On release builds (`TSESSION_PROFILE=release`), with
the machine quiet, before and after, with the numbers in the commit
message.

- `tools/dev/cost PID [SECONDS]`: the process's share of a core, its
  wakeups, and each window's frames with their cost. At rest: 0
  frames, 0.0%. Doing the thing you changed: compare with before.
- `tools/dev/bench [--apps a,b] [--scales 2]`: every app driven as
  people use it at 120 Hz, every frame against the 8.3 ms budget, and
  what each step cost the process (`cpu_ms`). **Compare two builds by
  that**, the processor's own time, not by their frames' marks: a mark
  is made by the build it measures, and two builds needn't count the
  same things (GPUI's left out what input handlers did; the kits'
  count the looper's work). For a wait the person feels (a click to a
  menu), compare what both show the same way: each client's Wayland
  log (`WAYLAND_DEBUG=client`), from the event to the commit.
- `tools/dev/scroll-check NAME --at X Y`: a drag, a flick and wheel
  notches, frame by frame: none skipped, steps as far as the fingers,
  how long the first move took to show.
- `OLO_FRAME_PHASES=1` prints where each frame's time went.
- Watch memory on anything that runs per frame (an atlas leak once
  took 35 GB).

## Before you commit

- [ ] Nothing on a looper can take longer than a frame; slow work
      shows progress, streams, and can be stopped.
- [ ] At rest the window draws nothing (`cost`: 0 frames).
- [ ] `tsession review` finds nothing at 1×, 1.5× and 2× (1.25× and
      1.75× where drawing depends on the scale), light and dark, with
      the scale changed while it ran and every tab visited.
- [ ] One picture of each batch of shots looked at with your own eyes.
- [ ] Agents can read and drive it: the tree names what's there.
- [ ] It comes back after `tsession reload`.
- [ ] What per-frame, per-event or idle work changed is measured
      before and after on release builds; the numbers are in the
      commit message.
- [ ] New widget or behaviour another app could use: in `crates/kit`,
      with an example, a lint test, and its paragraph in HACKING.md.
- [ ] `cargo build --release --workspace` (the person's session then
      offers it; never install or reload it yourself).
- [ ] The docs say what the code now does: HACKING.md, this book, and
      docs/TODO.md for what was left.
- [ ] A journal entry for anything visible (`tools/journal.py add`).

## From BeOS: a map

| BeOS | Olo | What changed, and why |
| --- | --- | --- |
| `BApplication` | `olo_kit::run`, the app's looper, `message_handler` | The same. |
| `BLooper`, `BHandler` | `olo_app::Looper`, `Handler`; a window's looper and its views | No `Lock()`: nothing outside a looper's thread touches what it owns, and the compiler checks it. |
| `BMessage` | a Rust type, most often an enum (`View::Message`) | Checked when it's built, not when a field is read. |
| `BMessenger` | `Messenger<M>`, `ViewRef<V>`, `WindowHandle<V>` | Never blocks; no synchronous reply on a looper. |
| `BMessageRunner` | `cx.after`, `cx.every`, the kit's `Due` | |
| `BWindow` | a window on a looper of its own (`open_window`) | A window that belongs to another can share its looper (`open_window_here`). |
| `BView` | `View` (state and messages) and `Element`s (what it shows) | BeOS had one object for both; here what's shown is described, kept and compared, so only what changed is laid out or drawn. |
| `BView::Draw` | `canvas`, `Painter` | Drawn when what it shows changed, not when it's exposed; kept as a display list. |
| `Invalidate()` | `cx.invalidate()` | |
| `MessageReceived` | `View::message` | |
| `QuitRequested` | `View::close_requested` | |
| app_server's thread for each window | the display thread and each window's stage | On the program's side (windows are Wayland surfaces), with the same effect: a busy window still scrolls and animates. |
| `BLayout` (Haiku) | flexbox on elements | |
| Scripting (`hey`), `BPropertyInfo` | the app's socket: menus, tree, invoke; `olo-screen` | What a person can see and do, an agent can read and drive. |
| Replicants, `BArchivable` | `save_state`, `restored_state`, upgrades in place | A program can be replaced under its windows. |
| The Interface Kit's controls | `olo_kit::widgets` | See the table in "The kit". |
