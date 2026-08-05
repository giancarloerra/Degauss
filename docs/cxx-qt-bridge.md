# cxx-qt Bridge Gotchas

Read this before writing Rust QML models with cxx-qt 0.8 in
`rust/frontend/src/models/`.

- **`cxx = "1"` must be a direct dependency.** The `#[cxx_qt::bridge]` macro
  expands to `#[cxx::bridge]`. Rust resolves proc-macro attributes in the
  calling crate's scope, so `cxx` must appear in that crate's `[dependencies]`.
  The transitive dependency through `cxx-qt` is not enough.

- **`#[qproperty(T, snake_case_name)]` keeps its Rust name** on the Qt and QML
  side. `#[qproperty(bool, has_next_page)]` generates
  `Q_PROPERTY(bool has_next_page READ getHas_next_page ...)`, so QML reads it as
  `model.has_next_page`. Only `#[cxx_name = "..."]` changes the exposed name.
  Reading a camelCase spelling yields `undefined`, which a `?? ""` fallback
  turns into a silent empty value rather than an error, so get the spelling
  right at the call site.

- **User-defined `#[qinvokable]` methods keep their Rust name** (snake_case).
  QML calls them as `model.set_system(id)` and so on. Add
  `#[cxx_name = "..."]` only when you need camelCase, such as matching a Qt
  base-class virtual like `rowCount`, `roleNames`, or `beginResetModel`.

- **The cxx-qt plugin class name** for `Zaparoo.Browse` is
  `Zaparoo_Browse_plugin`, not `Zaparoo_BrowsePlugin`. Use
  `Q_IMPORT_QML_PLUGIN(Zaparoo_Browse_plugin)` in the C++ entry point.

- **Bind QML singletons to data through `bind_to_endpoint!`.** QML singletons
  are constructed *after* `init_globals` runs. By then, the WebSocket task has
  usually moved past `Idle`/`Disconnected`. If `initialize()` only spawns an
  async watcher, the first frame can see the QObject's `Default::default()`
  placeholder values.

  The macro at `rust/frontend/src/bind.rs` emits the full
  `cxx_qt::Initialize` impl. It subscribes the singleton to the store-cached
  `RemoteResource<E::Output>` for the endpoint, reads the current
  `ResourceStatus` *synchronously* before returning, and only then spawns the
  `qt_thread` watcher for later updates. That makes the seed step part of the
  binding instead of something each model has to remember.

  ```rust
  // models/app_status.rs — full bridge for the AppStatus banner.
  crate::bind_to_endpoint! {
      for ffi::AppStatus,
      endpoint = CatalogEndpoint,
      args = (),
      select = project,       // fn(&ResourceStatus<CatalogData>) -> Projected
      apply  = apply_state,   // fn(Pin<&mut Self>, Projected)
  }
  ```

  `select` and `apply` are free functions, not closures, so they are `Copy`
  and can be reused by the sync seed and the async loop. For per-arg endpoints,
  such as `MediaSearchEndpoint` keyed by system id, call `Store::subscribe`
  from a `#[qinvokable]` and abort the previous watcher's `JoinHandle` before
  installing the next one (see `models/games.rs`).

  Use `tokio::sync::watch` for state exposed by `RemoteResource`.
  `tokio::sync::broadcast` drops messages sent before a receiver subscribes,
  which loses the seed value entirely (see the AGENTS.md "broadcast vs watch"
  note).
