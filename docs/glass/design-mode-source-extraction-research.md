# Design mode: in-page framework source extraction research

Research for the design-mode element picker (map #56, ticket #58): given a picked
DOM node, what component-name and source-file/line information can an injected
main-world script recover from dev-mode framework internals — no source maps, no
devtools extension, no build-tool cooperation. On production builds every field
must degrade silently to absent.

Evidence tiers used below: **[verified]** = read in the framework's source on
GitHub (tag or `main` as cited); **[documented]** = stated by official docs /
release notes / PR descriptions; **[inferred]** = deduced from the above,
not directly confirmed.

## 1. Summary

- **React ≤18 (dev)**: exact original source `fileName:lineNumber:columnNumber`
  per element via `fiber._debugSource` (compiled in by the dev JSX transform).
  **React 19 removed `_debugSource` entirely** (PR #28265, merged Feb 2024).
  The replacement is `fiber._debugStack` — an `Error` whose stack frames give
  the JSX callsite in *bundle/served-URL* coordinates, not original source
  (close to real files only under unbundled dev servers like Vite). Component
  *names* remain recoverable in all React dev versions via the fiber tree
  (`type.name` / `displayName`) and the `_debugOwner` chain.
- **Vue 3 (dev)**: component names (`type.name` / `type.__name`) and source
  **file only** (`type.__file`, full path in dev) via the non-enumerable
  `el.__vueParentComponent` instance and its `parent` chain. **No line numbers
  exist at runtime.** All of it disappears in prod builds unless the app opted
  into `__VUE_PROD_DEVTOOLS__`.
- **Svelte (dev)**: the best story. `element.__svelte_meta.loc` gives the
  **exact `.svelte` file, line, and column where the element was authored** —
  original source coordinates, better than React or Vue. Svelte 5 additionally
  chains `__svelte_meta.parent` (a `DevStackEntry` list) carrying
  `componentTag`, file, and line per enclosing component/block. Svelte 4 has
  the loc but no component names. Absent entirely in prod compiles.
- Production behavior: React prod fibers exist but carry no `_debug*` fields
  and minified names; Vue prod attaches nothing to elements; Svelte prod
  attaches nothing. The extractor's contract — probe, try/catch, omit — falls
  out naturally.

## 2. Orca baseline (`~/Projects/refs/orca/src/main/browser/grab-guest-script.ts`)

Orca's grab feature extracts **React only** — no Vue, Svelte, or other
framework handling exists anywhere in the file (grepped).

- `getFiberFromElement` (`:600-612`): scans `Object.keys(el)` for a key with
  prefix `__reactFiber$` **or** legacy `__reactInternalInstance$` and returns
  the fiber, try/catch-wrapped.
- `getComponentNameFromFiber` (`:614-626`): `fiber.type || fiber.elementType`;
  skips string types (host elements); reads `displayName || name`, falling
  back to `type.render` (forwardRef) and `type.type` (memo).
- `shouldSkipReactName` (`:628-633`): regex blocklist of framework-noise names
  (`Fragment`, `*Provider`, `*Boundary`, `*Router`, names ≤2 chars, etc.).
- `getReactMetadata` (`:651-682`): walks `fiber.return` up to depth 35,
  collecting ≤6 unique component names; takes the first
  `fiber._debugSource || fiber._debugOwner._debugSource` as
  `fileName:lineNumber[:columnNumber]`, cleaned by `cleanSourcePath`
  (`:635-649`, strips `webpack://`, `webpack-internal://`, `turbopack:///[project]/`,
  http origins, `file://`) and secret-scanned. Output: `reactComponents`
  (a string like `"<App> <Page> <Button>"`, outermost-first) and `sourceFile`
  (a single string), both clamped to 500 chars
  (`src/shared/browser-grab-types.ts:184-185`).
- Payload fields are **camelCase** (`reactComponents`, `sourceFile` —
  `src/shared/browser-grab-types.ts:67-68`). Glass's browser payloads use
  snake_case (`crates/browser/src/page_chrome.rs:17`
  `#[serde(rename_all = "snake_case")]`), so we do not copy orca's naming.
- Everything sits inside try/catch; on any failure it returns
  `{ reactComponents: null, sourceFile: null }`.

**Gaps vs. what we want:** orca relies on `_debugSource`, which is gone in
React 19 (its `_debugOwner._debugSource` fallback is equally gone); it has no
Vue/Svelte support; it flattens the chain into one display string instead of
structured entries; and it reports at most one source location for the whole
chain.

## 3. Per-framework findings

### 3.1 React

**DOM → fiber discovery.** react-dom stamps expando keys with a per-page-load
random suffix on every host node:
`const randomKey = Math.random().toString(36).slice(2)`, then
`__reactFiber$<suffix>` (host fiber), `__reactProps$<suffix>`,
`__reactContainer$<suffix>` (on root containers), plus `__reactEvents$`,
`__reactListeners$`, `__reactHandles$`, `__reactResources$`, `__reactMarker$`
and others **[verified:
[ReactDOMComponentTree.js@main](https://github.com/facebook/react/blob/main/packages/react-dom-bindings/src/client/ReactDOMComponentTree.js)]**.
These keys are plain enumerable assignments, so
`Object.keys(node).find(k => k.startsWith('__reactFiber$'))` works — this is
exactly what React DevTools-style tools and orca do. React's own
`getClosestInstanceFromNode` walks `node.parentNode` when the node has no key
**[verified: same file]** — our extractor should do the same DOM walk-up
(text nodes, portaled wrappers, and non-React islands have no key).

Version history of the key prefix:
- React ≤16.13: `__reactInternalInstance$<suffix>` (and a container key with a
  shipped typo, `__reactContainere$`) **[verified:
  [v16.13.1](https://github.com/facebook/react/blob/v16.13.1/packages/react-dom/src/client/ReactDOMComponentTree.js)
  lines 21-23]**.
- React 16.14 / 17+: renamed to `__reactFiber$<suffix>` **[verified: v16.14.0
  and
  [v17.0.2](https://github.com/facebook/react/blob/v17.0.2/packages/react-dom/src/client/ReactDOMComponentTree.js)]**.

Supporting both prefixes (as orca does) is one extra `startsWith` — keep it.
Both exist in prod builds too; the *fiber* is always reachable, only the debug
fields differ.

**`_debugSource` through React 18.** The dev JSX transforms compile a
`source` object into every element: classic runtime via
`@babel/plugin-transform-react-jsx-source` injecting `__source={{fileName,
lineNumber, columnNumber}}`, automatic runtime via `jsxDEV(type, props, key,
isStaticChildren, source, self)`. React 18 stores it on the element as the
DEV-only field `_source` (non-enumerable) and copies it to
`fiber._debugSource` **[verified:
[ReactJSXElement.js@v18.3.1](https://github.com/facebook/react/blob/v18.3.1/packages/react/src/jsx/ReactJSXElement.js)
— element factory fields `_self`/`_source`, `jsxDEV` passing `source` through]**.
These are *original source* coordinates (Babel/SWC/esbuild inject them before
bundling), which is why orca's `sourceFile` is exact on React ≤18 dev.

**React 19 removed it.** PR
[#28265 "Remove `__self` and `__source` location from elements"](https://github.com/facebook/react/pull/28265)
(merged Feb 7, 2024, shipped in 19.0) removed `_source`/`_debugSource` and the
rationale: compile-time locations only worked with JSX, weren't source-map
consistent, only captured the immediate callsite, and created DEV/PROD
inconsistencies **[documented: PR description]**. `jsxDEV` on `main` is now
`jsxDEV(type, config, maybeKey, isStaticChildren)` — extra `source`/`self`
args from older Babel output are silently ignored, and `createElement` strips
`__self`/`__source` props with an "outdated JSX transform" warning
**[verified:
[ReactJSXElement.js@main](https://github.com/facebook/react/blob/main/packages/react/src/jsx/ReactJSXElement.js)]**.
Community fallout and requests to restore it: issues
[#29092](https://github.com/facebook/react/issues/29092),
[#31981](https://github.com/facebook/react/issues/31981),
[#32574](https://github.com/facebook/react/issues/32574) — no restoration
planned as of writing.

**React 19 dev fields.** In DEV, fibers carry `_debugInfo`, `_debugOwner`,
`_debugStack`, `_debugTask`, `_debugNeedsRemount`, `_debugHookTypes`;
`_debugSource` does not appear anywhere in the file.
`createFiberFromElement` copies `element._owner → fiber._debugOwner` and
`element._debugStack → fiber._debugStack` **[verified:
[ReactFiber.js@main](https://github.com/facebook/react/blob/main/packages/react-reconciler/src/ReactFiber.js)]**.
`_debugStack` is created at JSX-call time as
`Error('react-stack-top-frame')` (with a raised `Error.stackTraceLimit`), or a
fake `UnknownOwner` stack when owner tracking is off **[verified:
ReactJSXElement.js@main]**. `_debugInfo` carries Server Components debug info
(virtual, not host-reachable per element — out of scope for v1).

**What React DevTools does post-19** — the authoritative recipe for source
recovery. DevTools' fiber renderer
(`packages/react-devtools-shared/src/backend/fiber/renderer.js` @main)
resolves an element's source lazily via `getSourceForFiberInstance`:

1. Take the `_debugStack` of any *owned child* of the component; the comment
   in `getSourceForInstance` explains: *"at the bottom of that stack will be a
   stack frame that is somewhere within the component's function body.
   Typically it would be the callsite of the JSX … This won't point to the top
   of the component function but it's at least somewhere within it."*
   It parses it with `parseStackTrace(fiber._debugStack, 1)` /
   `extractLocationFromOwnerStack` **[verified: renderer.js — the file handles
   `_debugStack` being an `Error` or an already-stringified stack]**.
2. Fallback: the "throwing trick" (`getSourceLocationByFiber`) — re-invoke the
   component in a throwing dispatcher context to capture a frame.

The coordinates in those frames are **served-URL coordinates**: under a
bundler dev server (webpack `eval-source-map`, etc.) they point into bundle
chunks / `webpack-internal://` URLs; under unbundled ESM dev servers (Vite)
the URL is essentially the real file path (`/src/App.tsx`) and the line is the
post-transform line, which esbuild's JSX transform keeps close to the original
**[inferred; the frame format itself is V8 `at fn (url:line:col)` —
CEF/Chromium only, so we need not handle SpiderMonkey/JSC formats]**.
Related: DevTools PR
[#28351](https://github.com/facebook/react/pull/28351) moved DevTools to this
lazy component-stack-based source model.

**`captureOwnerStack()`** — public dev-only API returning the owner stack as
`string | null` (`null` in prod and outside React-controlled contexts), added
in [React 19.1.0 (Mar 28, 2025)](https://github.com/facebook/react/releases/tag/v19.1.0)
**[documented: [react.dev/reference/react/captureOwnerStack](https://react.dev/reference/react/captureOwnerStack)]**.
Not usable from our injected script (we are not inside a React render/event
context), but confirms the frame-parsing approach is the sanctioned one.

**Component names (all versions).** Walk `fiber.return` from the host fiber;
for each fiber whose `type` is a function/object (FunctionComponent,
ClassComponent — host components have string `type`), read
`type.displayName || type.name`, with `type.render` (forwardRef) and
`type.type` (memo) fallbacks — orca's logic is correct and version-stable.
The `_debugOwner` chain (React 19 dev) gives the *creator* chain instead of
the tree parent chain — closer to "who wrote this JSX" — and is worth
preferring for attribution when present, falling back to the `return` walk.

**Dev vs prod detection.** Prod fibers are constructed without any `_debug*`
fields, so `('_debugOwner' in fiber)` distinguishes dev from prod
**[verified: `ReactFiber.js@main` initializes them under `if (__DEV__)`]**.
In prod, names from `type.name` are minified (1-2 chars) — orca's
`shouldSkipReactName` length filter already drops most; we should gate
explicitly: if the build is prod, emit `framework: "react"` at most, no
component entries.

**Next.js note (brief).** Next.js dev overlays consume React owner stacks and
do server-side source-map resolution; nothing extra is attached to DOM nodes
that beats `_debugStack` (page-level globals like `__NEXT_DATA__` identify the
framework, not the element) **[inferred]**. Orca's `cleanSourcePath` handling
of `turbopack:///[project]/` and `webpack-internal:///` prefixes covers the
Next dev URL schemes and should be ported.

### 3.2 Vue

**Vue 3 DOM → instance.** `mountElement` in the runtime renderer attaches, on
**every element it mounts**:

```ts
if (__DEV__ || __FEATURE_PROD_DEVTOOLS__) {
  def(el, '__vnode', vnode, true)
  def(el, '__vueParentComponent', parentComponent, true)
}
```

**[verified:
[renderer.ts@main](https://github.com/vuejs/core/blob/main/packages/runtime-core/src/renderer.ts)]**.
Two consequences:

- Dev builds (and prod builds that define the `__VUE_PROD_DEVTOOLS__` compile
  flag) have it; plain prod builds have neither property.
- `def` is `Object.defineProperty(..., { enumerable: false, ... })`
  **[verified: [shared/general.ts@main:152-160](https://github.com/vuejs/core/blob/main/packages/shared/src/general.ts)]**
  — so unlike React, **`Object.keys` will not find these**; probe the literal
  names `el.__vueParentComponent` / `el.__vnode` directly (they are fixed
  strings, no random suffix). Walk up the DOM for nodes Vue didn't mount.

**Component name.** From `instance = el.__vueParentComponent`, the component
definition is `instance.type`. Vue's own resolution
(`getComponentName`, component.ts@main:1217-1224 **[verified]**):
functions → `displayName || name`; options objects → `name || __name`.
`__name` is injected by compiler-sfc from the SFC **filename basename** when
no explicit name exists (`filename.match(/([^/\\]+)\.\w+$/)` →
`__name: '<basename>'`) **[verified: compileScript.ts@main]**; the SFC spec
documents filename-based name inference
**[documented: [vuejs.org/api/sfc-spec](https://vuejs.org/api/sfc-spec)]**.
`formatComponentName` additionally falls back to the basename of
`type.__file` **[verified: component.ts@main:1226-1237]** — copy that
fallback. Walk `instance.parent` for the enclosing component chain.

**Source file.** `type.__file` is attached by the build plugin, not the
compiler core. @vitejs/plugin-vue:

```ts
if (devToolsEnabled || (devServer && !isProduction)) {
  attachedProps.push([`__file`, JSON.stringify(isProduction ? path.basename(filename) : filename)])
}
```

**[verified:
[plugin-vue/src/main.ts@main](https://github.com/vitejs/vite-plugin-vue/blob/main/packages/plugin-vue/src/main.ts)]**
— full (project-relative) path in dev; in prod only a basename and only when
devtools support was explicitly enabled. vue-loader behaves equivalently
(`exposeFilename` option for prod basenames) **[documented]**.

**Line numbers: none.** Runtime vnodes and component instances carry no
source-location field — Vue's compiled render functions do not embed per-node
loc (loc exists only at compile time for codeframe errors). Expect **file
granularity only** **[inferred from renderer/vnode shape; no counter-evidence
found in vuejs/core]**.

**`__VUE_DEVTOOLS_GLOBAL_HOOK__`:** the hook object is *created by the
devtools extension*, not by Vue; Vue merely buffers events for 3s waiting for
one (`setDevtoolsHook`, devtools.ts@main **[verified]**). Without the
extension the global is absent — do not rely on it.

**Vue 2 (legacy tier).** `vm.$el.__vue__ = vm` is set in `_update` —
**unconditionally, in all builds including production** — and nulled on
destroy **[verified:
[vuejs/vue src/core/instance/lifecycle.ts:79-85,136-139](https://github.com/vuejs/vue/blob/main/src/core/instance/lifecycle.ts)]**.
Caveat: only on **component root elements**, so a DOM walk-up is required.
Names via `vm.$options.name` (a string literal — survives minification, so
prod Vue 2 can still yield real names); `vm.$options.__file` dev-only via
vue-loader. Vue 2 is EOL (Dec 2023); support = the same walk-up plus two
property reads, worth the ~10 lines.

### 3.3 Svelte

**Svelte 3/4, compiled with `dev: true`.** The compiler emits an
`add_location` call per element:

```js
export function add_location(element, file, line, column, char) {
  element.__svelte_meta = {
    loc: { file, line, column, char }
  };
}
```

**[verified:
[packages/svelte/src/runtime/internal/utils.js@svelte@4.2.19:34-38](https://github.com/sveltejs/svelte/blob/svelte%404.2.19/packages/svelte/src/runtime/internal/utils.js)]**
(Svelte 3 has the identical helper in `internal/dom.ts` **[documented]**).
`file` is the compiler's `filename` option (vite-plugin-svelte passes the
project-relative path); `line`/`column` are **original `.svelte` source
coordinates** of the element's tag — exact file/line, better than anything
React or Vue offers. No component name is attached in Svelte 3/4. The `dev`
compile option gates this (enabled by vite-plugin-svelte during `vite dev`,
off for builds) **[documented:
[svelte.dev compiler options](https://svelte.dev/docs/svelte/svelte-compiler)]**.

**Svelte 5, dev.** Templates are wrapped `if (dev)` in
`$.add_locations(template, Component[$.FILENAME], locations)` — dev templates
are deliberately never deduplicated so per-callsite locations stay accurate
**[verified:
[transform-template/index.js@main:73-80,95-96](https://github.com/sveltejs/svelte/blob/main/packages/svelte/src/compiler/phases/3-transform/client/transform-template/index.js)]**.
The runtime assigns:

```js
element.__svelte_meta = {
  parent: dev_stack,
  loc: { file: filename, line: location[0], column: location[1] }
};
```

**[verified:
[internal/client/dev/elements.js@main](https://github.com/sveltejs/svelte/blob/main/packages/svelte/src/internal/client/dev/elements.js)
— `assign_locations` also handles SSR-hydrated trees via hydration-comment
walking]**. Note Svelte 5 drops the `char` field that Svelte 4 had.

**Component names in Svelte 5 — yes.** `dev_stack` entries are:

```ts
interface DevStackEntry {
  file: string;
  type: 'component' | 'if' | 'each' | 'await' | 'key' | 'render';
  line: number;
  column: number;
  parent: DevStackEntry | null;
  componentTag?: string;
}
```

**[verified:
[internal/client/types.d.ts@main:203-210](https://github.com/sveltejs/svelte/blob/main/packages/svelte/src/internal/client/types.d.ts)]**,
built by `add_svelte_meta(callback, type, component, line, column, additional)`
with `file: component[FILENAME]` **[verified: internal/client/context.js@main:36-53]**.
Component mounts pass `{ componentTag: node.name }` **[verified:
[visitors/shared/component.js@main:509](https://github.com/sveltejs/svelte/blob/main/packages/svelte/src/compiler/phases/3-transform/client/visitors/shared/component.js)]**,
and the compiler emits `App[$.FILENAME] = 'src/App.svelte'` in dev
**[verified: transform-client.js@main:534-540]**. So walking
`__svelte_meta.parent` and filtering `type === 'component'` yields
`{componentTag, file, line, column}` per enclosing component — name *and*
instantiation site. (`FILENAME` is an unexported `Symbol`, but we never need
it directly; the strings are already denormalized into the entries.)

**Production:** the `add_locations`/`add_location` wrapping is emitted only
under `dev` — no `__svelte_meta` on any element, silent absence **[verified:
the `if (dev)` guards above]**.

## 4. Feasibility matrix

| Framework / tier | Component name | Source file | Line/col | Prod-build failure mode |
|---|---|---|---|---|
| React ≤16.13 dev | Yes — fiber walk via `__reactInternalInstance$*`, `type.displayName/name` | Yes, exact — `fiber._debugSource.fileName` (needs dev JSX transform) | Yes, exact | Fiber present, no `_debug*`, minified names → emit nothing |
| React 16.14–18 dev | Yes — same via `__reactFiber$*` | Yes, exact — `_debugSource` | Yes, exact | Same as above |
| React 19+ dev | Yes — fiber walk + `_debugOwner` chain | Best-effort — parse bottom frame of `_debugStack` (Error) → served-URL path; near-real under Vite, bundle-internal under webpack | Best-effort, bundle coords; mark as approximate | Same as above; `_debugStack` absent in prod |
| Vue 3 dev | Yes — `el.__vueParentComponent.type` name/`__name`, `parent` chain | Yes — `type.__file` full path (plugin-attached) | **No** | Properties never attached (unless app set `__VUE_PROD_DEVTOOLS__`) → probe fails silently |
| Vue 3 prod + `__VUE_PROD_DEVTOOLS__` | Partial — `name`/`__name` survive (string), else minified | Basename only, and only if plugin `devToolsEnabled` | No | n/a (this *is* prod) |
| Vue 2 (legacy) | Yes — `$el.__vue__.$options.name` (works even in prod if `name` set) | Dev only — `$options.__file` | No | `__vue__` still present; `__file` absent; name may be absent |
| Svelte 3/4 dev | **No** | Yes, exact — `__svelte_meta.loc.file` | Yes, exact (`line`, `column`, `char`) | `__svelte_meta` absent → silent |
| Svelte 5 dev | Yes — `__svelte_meta.parent` chain `componentTag` | Yes, exact — `loc.file` + per-component `file` | Yes, exact (`line`, `column`) | `__svelte_meta` absent → silent |

Reliability notes: Svelte dev is the most reliable (data authored for exactly
this purpose, original coordinates). Vue dev is reliable but file-only.
React ≤18 dev is exact but requires the app to use the dev JSX transform
(virtually all toolchains do). React 19 source is the only *fragile* cell:
it depends on parsing V8 stack strings and on how the dev server serves code.

## 5. Recommended payload fields

Extend the pick payload (glass uses `#[serde(rename_all = "snake_case")]`,
`crates/browser/src/page_chrome.rs:17`) with optional fields — every field
best-effort, omitted (not nulled) when unavailable:

```jsonc
{
  // ...existing pick payload fields...
  "framework": "react",            // "react" | "vue" | "svelte"; omitted if none detected
  "framework_dev": true,           // dev-mode internals were present
  "components": [                  // nearest-first, cap 6 entries (orca precedent), each field optional
    {
      "name": "SubmitButton",           // component name / tag
      "source_file": "src/lib/SubmitButton.svelte", // cleaned path (orca cleanSourcePath rules)
      "source_line": 42,                // 1-based, only when known
      "source_column": 8,               // only when known
      "source_precision": "exact"       // "exact" (source coords) | "bundle" (React 19 stack frames)
    }
  ],
  "element_loc": {                 // the picked element itself, when the framework provides it
    "source_file": "src/lib/SubmitButton.svelte",  // Svelte: loc.file; React ≤18: _debugSource of host fiber
    "source_line": 45,
    "source_column": 2
  }
}
```

Rules:

- **Silent degradation:** the extractor wraps every property access in
  try/catch; any failure or prod build yields an omitted field, never an
  error, never a placeholder string. Rust side: all fields
  `Option<...>`/`#[serde(default)]`, clamped like `parse_page_chrome_payload`.
- `source_precision` keeps the React 19 bundle-coordinate caveat honest in the
  UI ("near `bundle.js:1201`" vs "SubmitButton.svelte:45").
- Budgets: cap names and paths (orca uses 500 chars total per field); cap the
  chain at 6; cap fiber/instance/stack walks (orca: depth 35).
- Detection order per node (cheapest, fixed-name probes first):
  `__svelte_meta` → `__vueParentComponent` → `__vue__` → `Object.keys` scan
  for `__reactFiber$`/`__reactInternalInstance$`; first hit wins for that
  node; walk up the DOM (bounded, e.g. 10 ancestors) before giving up.
  Micro-frontend pages can mix frameworks, so detect per picked node, not per
  page.

## 6. Open questions / risks

1. **React 19 stack-frame parsing fragility.** `_debugStack` is an `Error`
   (or occasionally a pre-stringified stack — DevTools handles both; we
   should too). CEF pins us to V8's `at name (url:line:col)` format, but
   frames include React-internal top frames (`react-stack-top-frame`,
   `UnknownOwner` fakes) that must be skipped, and async/eval frames vary.
   Recommend: port the *idea* of DevTools' `extractLocationFromOwnerStack`
   (bottom user frame of an owned child's stack) but treat output as
   `source_precision: "bundle"` and accept misses.
2. **Bundle vs source coordinates.** Under Vite dev the served URL ≈ real
   file and lines are close; under webpack/Next the path is
   `webpack-internal://` noise. Orca's `cleanSourcePath` prefixes cover the
   common schemes; anything still opaque after cleaning should arguably be
   dropped rather than shown.
3. **Owner chain vs tree chain (React).** `_debugOwner` gives "who created
   this JSX" (better attribution); `fiber.return` gives tree context. v1 can
   ship the `return` walk (orca-proven) and add owner-chain preference later.
4. **Multiple React roots / multiple renderer copies** each stamp their own
   random-suffix key; per-node `Object.keys` scanning handles this for free.
   Two Reacts on one page each mark their own subtrees.
5. **Svelte 5 `parent` chain growth.** `dev_stack` entries include control
   blocks (`if`/`each`/...) between components; filter to
   `type === 'component'` and cap the walk.
6. **Vue apps mounted without a build step** (CDN global build, in-browser
   template compilation): dev build still attaches `__vueParentComponent`,
   but `__file`/`__name` don't exist (no SFC compiler) — name falls back to
   explicit `name` options or nothing. Expected, degrades correctly.
7. **Detection order vs false positives.** A page property named
   `__svelte_meta` could be forged by page JS; the payload is display/context
   data, not a capability (same trust posture as the existing picker payload,
   see `design-mode-picker-research.md` §risks), so forgery is accepted.
8. **`_debugInfo` / Server Components.** RSC debug info is attached to
   fibers as virtual-instance data; surfacing "which server component
   rendered this" is possible in principle but DevTools-grade work — defer.
