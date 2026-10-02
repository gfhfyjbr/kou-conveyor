# Canvas — холст агентов, терминалов и событий

> План интеграции в kou-conveyor (браузерный кокпит). Черновик v1, 2026-10-02.
> Работа ведётся по фазам (§14). Всё, что касается внешних CLI (Claude Code
> 2.1.x, Codex 0.160, OpenCode), помечено «**проверить в спайке**»: флаги этих
> программ меняются быстрее нашего кода, поэтому они вынесены в данные
> пресетов, а не захардкожены.

## Содержание

0. [Коротко](#0-коротко)
1. [Что уже есть и на что опираемся](#1-что-уже-есть-и-на-что-опираемся)
2. [Цели, не-цели, принципы](#2-цели-не-цели-принципы)
3. [UX](#3-ux)
4. [Модель данных и хранение](#4-модель-данных-и-хранение)
5. [Сервер: движок канваса](#5-сервер-движок-канваса)
6. [Терминальные ноды](#6-терминальные-ноды)
7. [Агентные ноды (харнесы)](#7-агентные-ноды-харнесы)
8. [Связи и маршрутизация](#8-связи-и-маршрутизация)
9. [Управление канвасом агентами](#9-управление-канвасом-агентами)
10. [События и плагины](#10-события-и-плагины)
11. [Веб-плагин `canvas`](#11-веб-плагин-canvas)
12. [Безопасность и ограждения](#12-безопасность-и-ограждения)
13. [Изменения в существующем коде](#13-изменения-в-существующем-коде-и-почему-пайплайн-не-ломается)
14. [Фазы работ](#14-фазы-работ)
15. [Тестирование](#15-тестирование)
16. [Риски и открытые вопросы](#16-риски-и-открытые-вопросы)
17. [Приложения](#17-приложения)

---

## 0. Коротко

- **Что это.** Канвас — бесконечная доска с нодами:
  - **терминалы**: просто шелл, шелл с командой или харнес в PTY (Claude Code, Codex, OpenCode…);
  - **kou-агенты**: упрощённая сессия нашего харнеса, то есть веб-лента и инпут, а не терминал;
  - **источники событий**: GitHub issues, таймер, файлы, webhook — всё дополняется плагинами;
  - **заметки**.

  Ноды соединяются рёбрами «выход → вход», по рёбрам ходят сообщения.
- **Как это встаёт в проект.** Всё новое кладём рядом со старым:
  - встроенный веб-плагин `canvas` — вид со своей страницей, как Accounts или пример `scratchpad`;
  - серверный движок `cmd/internal/canvas`;
  - маршруты `/api/w/{ws}/canvases…`;
  - CLI `kou-conveyor-canvas` (в терминалах доступен как `kou-canvas`).

  Сессии, раннер, очередь, таймлайн и композер ведут себя как раньше. В существующем коде появляются только точки расширения, которые по умолчанию ничего не делают (§13). Выключил плагин `canvas` в Plugins или запустил сервер с `-canvas=off` — кокпит ровно такой же, как сейчас.
- **Вход в канвас.** New session → вверху сегмент **Chat · Canvas**. Выбор Canvas превращает свежую сессию в канвас: тот же UUID, адрес `#/w/<ws>/c/<id>`.
- **Ввод в терминал** работает как `tmux send-keys` / `paste-buffer -p`:
  - вставка текста (bracketed paste, если программа его включила — сервер уже отслеживает режим 2004), затем Enter с настраиваемой задержкой;
  - именованные клавиши (`C-c`, `Escape`, `Up`…).

  Человек и агент работают в одном PTY.
- **Что считается выходом ноды.**

  | Нода | Выход |
  |---|---|
  | шелл | вывод команды между метками OSC 133 C…D |
  | Claude Code | хуки, переданные флагом `--settings`: `Stop` даёт последний ответ |
  | Codex | `notify` с событием `agent-turn-complete` |
  | kou-агент | финальный ответ рана (хук в `pump`) |
  | прочие TUI | эвристика простоя |

- **Агенты управляют канвасом через один HTTP API** с токеном ноды. К API ведут три пути:
  - CLI `kou-canvas`, он есть в PATH каждого канвас-терминала;
  - MCP stdio-сервер `kou-canvas mcp` для Claude Code и Codex;
  - инструменты kou-агентов — встроенный harness-плагин `canvas-agent`, активный только при `KOU_CANVAS_TOKEN`.

  Агенты видят граф с координатами, создают и удаляют ноды (терминал с командой, в worktree; kou-агента), соединяют и двигают их, шлют текст и клавиши, читают экран и ждут завершения.
- **События — это плагины.** В `plugin.json` появляется секция `canvas` с полями `harnesses`, `sources`, `templates`. Источник — процесс, который печатает JSONL-события (stream или poll). Сервер следит за процессом, отбрасывает дубли и раскладывает события по рёбрам.
- **Фазы:** 0 спайки → 1 каркас → 2 рёбра и роутер → 3 харнесы → 4 API агентов и Бригадир → 5 события и плагины → 6 полировка.

---

## 1. Что уже есть и на что опираемся

| Факт | Где | Что это даёт канвасу |
|---|---|---|
| Страница — хост плагинов. Всё UI (layout, сессии, композер, сайдбар) — плагины на одном API: слоты, точки вклада, сервисы, хуки, роуты, `hot.data` | `cmd/kou-conveyor-web/static/kernel/host.js`, `docs/plugins.md` | Канвас — ещё один встроенный плагин, выключаемый и перезагружаемый на лету |
| Вид со своей страницей: `layout.view { id, title, rail, page, select }`. При `page` layout прячет bar, scroll, dock и боковые панели (`.shell[data-page]`) | `plugins/layout/web/layout.js`, `layout.css`; пример `examples/plugins/scratchpad` | Страница канваса без единой правки layout |
| Роуты идут по приоритету. Роут сессий (приоритет 0) матчит всё: `#/w/<ws>/s/<id>`, `#/w/<ws>` | `plugins/session/web/session.js` → `routeSessions` | Роут `#/w/<ws>/c/<id>` с приоритетом 10, как `#/notes` у scratchpad |
| Модель сессии: один `view()` за раз. `fresh` — ещё не сохранена; есть `newSession()` и `ensureView()` | `session.js` | Chat · Canvas виден только у `fresh`. Модель сессии канвас не трогает |
| Слоты `bar.crumbs`, `bar.end`, `dock` (resume 10, activity 20, queue 30, composer 40) | `layout.js`, `header.js`, `composer.js` | Переключатель Chat · Canvas монтируется в `bar.crumbs` |
| Список сессий строится из `session.state.sessions` | `plugins/session-list/web/session-list.js` | Нужна точка `session-list.rows` для канвасов и скрытие дочерних сессий |
| Терминалы — PTY на сервере. Переживают перезагрузку страницы и самопересборку сервера (`Handover`/`Adopt`). WS: binary = ввод, `{"type":"input"}`, `{"type":"resize"}`; сервер шлёт `hello`, replay, `live`, `meta`, `exit` | `cmd/internal/terminal/*`, `cmd/kou-conveyor-web/terminals.go` | Канвас-терминалы — те же сессии `terminal.Manager`, только с владельцем |
| Лимиты: 64 шелла на сервер, кольцо вывода 2 MiB, очередь клиента 1024 сообщения | `terminal.go` | Учесть в лимитах канваса |
| Сканер вывода знает OSC 0/2 (title), OSC 7 (cwd) и DEC-режимы, включая **2004** (bracketed paste) и 1049 | `cmd/internal/terminal/scan.go` | Знаем, оборачивать ли вставку в `ESC[200~…ESC[201~` |
| Интеграция шеллов: zsh шлёт OSC 133 A/B/C/D;код, bash — только A/B, fish — ничего | `plugins/terminal/shell/{zsh,bash,fish}` | Статус и вывод shell-ноды по OSC 133. Для bash и fish дописать C/D |
| restty (libghostty-vt в WASM, WebGPU/WebGL2): у каждого экземпляра свой GPU-контекст; есть `autoResize`; размер берётся из `getBoundingClientRect` | `plugins/terminal/web/view.js`, `vendor/restty.esm.js` | LOD и «живые» терминалы только при масштабе около 1 (спайк 0.1) |
| Очередь сессии на сервере: `POST /sessions/{id}/queue` — при простое запуск сразу, при ране — ждать, `force` — steer в идущий ран | `cmd/kou-conveyor-web/queue.go` → `handleEnqueue` | Доставка в kou-ноды. Ядро выносим в метод, поведение то же |
| Ран: `launch` → `pump` → событие `done`. События рана — SSE `/api/runs/{id}/events` с `Last-Event-ID` | `runs.go` | Хуки «ран начался/закончился» → статус и выход kou-ноды |
| Env раннера собирается в `cockpit.Start`. Запрос — JSON с `RejectUnknownMembers`. Возможности раннера определяются по `-h` (`featuresOf`) | `cmd/internal/cockpit/runner.go`, `cmd/internal/agentrunner/run.go` | Канвас-ранам добавляем env, протокол раннера не меняем |
| Sandbox раннера `worktree`: `.harness/worktrees/<session8>`, ветка `kou/<session8>`; режим задаёт `KOU_CONVEYOR_SANDBOX` | `cmd/internal/agentrunner/sandbox.go` → `ensureWorktree` | kou-нода «в worktree» — это env sandbox=worktree. Для терминалов тот же хелпер, вынесенный в пакет |
| Метаданные сессии кокпита: `.meta/<id>.json` (Title, Pinned, PinOrder) | `cmd/internal/cockpit/manage.go` | Добавляем `canvas` и `node`: так агентская сессия знает свой канвас |
| Плагины: `plugin.json` (`tools` — команды с JSON на stdin, `skills`, `prompt`, `commands`, `web`), строгий парсинг. Веб-встроенные плагины видит только web-сервер (`Options.Builtins`), раннер видит `harness/plugin/builtin/*`, user и workspace | `harness/plugin/*` | Секция `canvas` в манифесте. Инструменты агентов — harness-встроенный `canvas-agent` |
| Защита: allowlist Host, `CrossOriginProtection` на не-GET, CSP `connect-src 'self' ws://host` | `server.go` → `secure`, `handler` | Агентский API — bearer-токены, браузерный — как сейчас |
| Самопересборка ждёт `busy()` (активные раны, непустые очереди), передаёт терминалы и сокет, делает `exec` | `rebuild.go` → `busy`, `takeUp` | Документы канвасов сбрасываются на диск перед `exec`, источники перезапускаются |

Из этого следует: 80 % нужного уже есть. PTY с переживанием рестартов, очередь со steer, worktree-sandbox, плагинная система и механизм «вида со страницей» — готовые кирпичи. Канвас — это оркестрация поверх них, а не новый рантайм.

---

## 2. Цели, не-цели, принципы

### Цели

1. Бесконечный канвас с нодами: терминалы (в том числе с чужими харнесами), упрощённые kou-сессии, события, заметки.
2. Входы и выходы нод и рёбра между ними задают, откуда нода получает информацию.
3. Ввод в терминал «как tmux/zellij»: вставка, набор и клавиши.
4. Полноценный терминал: в нём одновременно работают человек и подключённый агент.
5. Event-ноды из плагинов (GitHub Issue Eventer и т. п.), на которые реагируют связки агентов.
6. Агенты (Claude Code, Codex, kou) создают и удаляют ноды и терминалы, настраивают связи, видят координаты, создают терминалы с командой и в worktree.
7. Вход через New Session: табы Chat · Canvas.
8. Основной пайплайн не ломается: канвас — отдельный вид и отдельный модуль.

### Не-цели v1

- Совместное редактирование несколькими людьми. Несколько вкладок одного пользователя поддерживаем (LWW).
- Канвас в TUI (`kou-conveyor-tui`).
- No-code язык с условиями и циклами. Будут только шаблоны текста и простой фильтр (фаза 6).
- Вебхуки из интернета без туннеля.
- Виндовс: канвас поддерживает те же платформы, что пакет `terminal`.

### Принципы

- **Аддитивность.** Новое — это плагин, пакет и маршруты. Существующие файлы получают только точки расширения с нулевым эффектом по умолчанию.
- **Сервер — источник истины**, как для ранов и терминалов. Закрыл вкладку — источники ловят события, агенты работают, терминалы живы. Страница только показывает.
- **Один API на всех.** UI, CLI, MCP и инструменты kou — клиенты одной модели операций (§4.4).
- **Расширение плагинами.** Пресеты харнесов, источники событий, шаблоны и рендереры нод — это данные и код плагинов.
- **В файлы пользователя ничего не пишем.** Так уже устроена интеграция шеллов. Хуки и MCP для Claude Code и Codex передаются флагами запуска, а не правкой `~/.claude` и `~/.codex`.
- **Видимость и тормоза.** Каждое действие агента видно (лента Activity, подсветка). Есть лимиты, пауза канваса и подтверждения на рёбрах.
- **Один писатель на рабочую копию.** Это вывод `audit-234b16c9/DESIGN-long-running.md` §5.2. Параллельные «писатели» работают в разных worktree, и пресеты подталкивают к этому по умолчанию.

---

## 3. UX

### 3.1 Вход: New session → Chat · Canvas

- **Где.** В баре стадии (слот `bar.crumbs`, после крошек) — сегментный контрол `[ Chat | Canvas ]` (`role="tablist"`). Он виден только когда:
  - активен вид Sessions и `session.view().fresh` (новая, ещё не сохранённая сессия);
  - или открыта страница канваса, который ещё «свежий»: без нод и не сохранён.

  Дальше сессия «стала» чатом или канвасом, и переключатель исчезает.
- **Chat** (по умолчанию) — обычный пустой экран и композер, ничего не меняется.
- **Canvas:**
  - выполняется `history.pushState('#/w/<ws>/c/<uuid свежей сессии>')`, роут показывает вид `canvas`;
  - черновик из композера, если он был, переносится в поле Бригадира (§9.7);
  - свежий канвас ничего не пишет на диск до первой ноды или первого промпта — так же, как свежая сессия до первого рана.
- **Назад в Chat** возвращает тот же свежий вид сессии: `session.ensureView()` держит его вместе с черновиком.
- **Другие входы:** `/canvas [название]`, палитра (⌘K) → «New canvas», вкладка Canvas в рейле, строка канваса в списке сессий.
- **Названия.** Основной вариант — **Chat · Canvas**. Альтернативы в духе «конвейера»: *Solo · Crew*, *Line · Floor*. Внутреннее имя вида — `canvas`.

### 3.2 Страница канваса

```
┌ рейл (вид canvas) ─────────┐┌ стадия: страница канваса ────────────────────────────────────────┐
│ ← Sessions                 ││ unreal-agent / Issue triage ✎  [Chat|Canvas]*  ● Live  ⚙2 ⏸1  ＋ ⌖ 100% ⋯│
│ CANVASES            + New  ││ ┌─────────────────┐          ┌──────────────────┐                   │
│ ◧ Issue triage   live  2m  ││ │ ⚡ GitHub issues │──○──────▶│ ◆ Triage · kou    │──○─────┐          │
│ ◧ Pair review          1h  ││ │ owner/repo   ✓  │          │ idle · claude-opus│        │          │
│ OUTLINE                    ││ └─────────────────┘          └──────────────────┘        ▼          │
│  ⚡ GitHub issues   ✓ 2m    ││                                         ┌─────────────────────────┐ │
│  ◆ Triage          idle    ││                                         │ ✻ Claude Code · coder    │ │
│  ✻ Claude · coder  busy ●  ││                                         │ ⎇ kou/fix-42  [restty…]  │ │
│ ACTIVITY                   ││                                         └─────────────────────────┘ │
│  12:01 #42 → Triage        ││              ┌──────────────────────────────────────┐               │
│  12:02 Triage spawned ✻    ││              │ Ask the foreman…              ⏎      │               │
└────────────────────────────┘└───────────────────────────────────────────────────────────────────┘
  * только пока канвас свежий
```

- **Рейл** (`layout.view.rail`): возврат к сессиям, список канвасов воркспейса, Outline текущего (ноды со статусами; клик центрирует ноду) и Activity — сообщения по рёбрам и действия агентов.
- **Шапка страницы** — свой `header.bar` внутри `page`, как у scratchpad:
  - название (клик — переименовать);
  - Live/Paused (§8.6);
  - счётчики «работают» и «ждут человека»;
  - кнопки `+ Add` (меню видов и пресетов), `Fit`, масштаб, `⋯` (экспорт, сохранить как шаблон, удалить);
  - «Pop out» — открыть канвас в отдельном окне (`window.open` того же адреса).
- **Поверхность** — бесконечная сетка (шаг 8 px), ноды, рёбра, позже мини-карта.
- **Инспектор** — правый выезжающий оверлей внутри страницы со свойствами выделенной ноды или ребра. Формы строятся по JSON-схемам видов и пресетов (§10.1).
- **Поле Бригадира** — внизу по центру (§9.7).

### 3.3 Виды нод

| Вид (`kind`/`preset`) | Глиф | Что внутри | Порты | Размер по умолчанию |
|---|---|---|---|---|
| `terminal` / `shell` | ▣ | логин-шелл с интеграцией, restty | in `in` (ввод), out `out` (вывод команды), out `exit` | 760×460 |
| `terminal` / `command` | ▣ | шелл и команда, набранная в нём (видна в истории) | то же | 760×460 |
| `terminal` / `claude-code`, `codex`, `opencode`, … | ✻ ◎ ⌬ | харнес в PTY и адаптер пресета (§7.2) | in `in` (промпт), out `out` (ответ за ход) | 760×520 |
| `agent` (kou) | ◆ | упрощённая kou-сессия: компактная лента и инпут | in `in`, out `out` | 440×560 |
| `source` / `<плагин>:<id>` | ⚡ | состояние источника, последние события, форма настроек | только выходы — по `outputs` источника | 320×220 |
| `note` | ¶ | markdown-заметка | нет | 280×180 |

Позже (фаза 6): `frame` (группа нод), `transform` (шаблон или фильтр как отдельная нода).

Анатомия ноды:

```
┌─ ✻ Claude Code · coder ─────────────── ● busy  12s ─ ⋯ ┐
○ in                                                   out ○   ← порты на левой/правой кромке
│  [поверхность restty / лента kou / лента событий]        │
│                                                          │
├─ ⎇ kou/coder · ~/repo/.harness/worktrees/coder · ✻ claude ┤   ← подвал: worktree, cwd, программа
└──────────────────────────────── 2 pending · from Triage ─┘   ← бейдж очереди доставки
```

### 3.4 Статусы нод

| Статус | Как выглядит | Откуда берётся |
|---|---|---|
| `starting` | серый, крутилка | процесс запускается |
| `idle` | приглушённый | шелл на приглашении (133;A), хук `Stop` / `notify`, ран завершён |
| `busy` | акцент, «бегущий» индикатор | команда идёт (133;C), `UserPromptSubmit`, ран идёт, вывод активен |
| `waiting` | `--warn` | харнес ждёт человека: хук `Notification` (разрешение), approve-сообщение |
| `paused` | штриховка | канвас или нода на паузе |
| `error` | `--err` | ран упал, источник падает, срабатывание предохранителя |
| `exited` | серый, код выхода | процесс завершился, кнопка «Restart» |
| `stopped` | серый | процесса нет (рестарт сервера без handover); кнопки «Start» / «Resume» |

### 3.5 Связи

- **Порты.** Входы на левой кромке, выходы на правой. Тянем от выхода к входу — получаем ребро; неподходящие цели гаснут.
- **Ребро** — кривая Безье. При прохождении сообщения по нему бежит точка, на ребре есть счётчик за сессию. Клик выделяет ребро, в инспекторе:
  - шаблон с предпросмотром на последнем сообщении;
  - режим: `auto` | `approve` | `off`;
  - доставка при занятой цели: `queue` | `now`;
  - «заголовок происхождения» on/off;
  - последние 20 сообщений по ребру.
- **`approve`** — сообщения ждут на целевой ноде («1 pending — Approve / Edit / Drop»). По умолчанию так включаются рёбра агент→агент, образующие цикл (§8.5).
- **Очередь доставки** — бейдж «N pending» на ноде; клик открывает список, где можно удалить или поменять порядок.

### 3.6 Канвасы в списке сессий

- Канвас — строка в списке Sessions: глиф `◧` вместо номера, название и мета вида `canvas · 3 agents · live · 2m`. Сортируется по времени вместе с сессиями. Работает через новую точку `session-list.rows` (§13), без канвас-плагина её нет.
- Сессии kou-агентов канваса (у них `meta.canvas`) по умолчанию скрыты из плоского списка: они видны на самом канвасе. В фильтре есть переключатель «Show canvas agents».
- Если открыта полная сессия такого агента (кнопка ноды «Open session»), в крошках появляется `◧ Issue triage ▸ Triage` — ссылка назад на канвас.

### 3.7 Терминал: человек и агент в одном PTY

- Терминал полноценный: клик или Enter по ноде переводит фокус в PTY (`data-keys="own"`, как в сайдбаре), и человек печатает как обычно.
- Ввод агента (CLI, MCP, рёбра) атомарен: одна запись — одна вставка. Пока агент пишет и ещё ~1.5 с после, на ноде висит плашка «⌨ Foreman typing…».
- Режим ввода ноды (`input`): `shared` (по умолчанию) | `human` (агенты не пишут, сообщения копятся в pending) | `agent` (клавиатура человека выключена, просмотр разрешён).
- По умолчанию агент доставляет текст только когда нода `idle`. Опция `now` — печатать сразу.

### 3.8 Клавиши (только на виде `canvas`, `views: ['canvas']`)

| Клавиша | Действие |
|---|---|
| Space + перетаскивание, средняя кнопка, прокрутка двумя пальцами | панорама |
| ⌘/Ctrl + колесо, щипок | масштаб |
| ⌘0 / ⌘1 / ⌘= / ⌘− | вписать всё / 100 % на выделении / ближе / дальше |
| `A` | меню «Add» в точке курсора |
| Delete / Backspace | удалить выделенное (с тостом Undo, терминал живёт 15 с — §6.3) |
| ⌘D | дублировать |
| ⌘Z / ⌘⇧Z | отменить / вернуть свои операции |
| Enter | войти в ноду: терминал → фокус PTY, kou → инпут |
| ⌘Esc (`own: true`) | выйти из ноды на канвас (Esc нужен самим TUI — Claude Code прерывается по Esc) |
| Tab / ⇧Tab | следующая / предыдущая нода |
| F | режим фокуса: нода на 100 % по центру |

Новых глобальных однобуквенных клавиш не добавляем (см. `UX-AUDIT.md`): клавиши работают только на виде канваса.

### 3.9 Узкий экран

Кокпит «помещается в телефон». Канвас на ширине < 720 px показывается в режиме Outline: вертикальный список нод; тап открывает ноду на весь экран — терминал, ленту kou или журнал источника. Панорама и масштаб доступны, но вторичны.

---

## 4. Модель данных и хранение

### 4.1 Документ канваса

```jsonc
{
  "version": 1,
  "id": "6b1f0c2e-…",                 // UUID; совпадает с UUID свежей сессии, из которой создан
  "title": "Issue triage",
  "created_at": "2026-10-02T12:00:00Z",
  "updated_at": "2026-10-02T12:05:00Z",
  "rev": 42,                          // растёт на каждый применённый пакет операций
  "live": true,                       // источники работают, маршрутизация включена (§8.6)
  "settings": {
    "autonomy": { "spawn": "allow", "max_nodes": 48, "max_terminals": 16, "spawns_per_minute": 6, "max_depth": 3 },
    "routing":  { "max_hops": 32, "edge_rate_per_minute": 30, "canvas_rate_per_minute": 120 }
  },
  "nodes": [
    { "id": "n_k3d9qa", "kind": "source", "preset": "github-events:github-issues", "plugin": "github-events",
      "title": "GitHub issues", "x": 0, "y": 0, "w": 320, "h": 220, "z": 1,
      "config": { "repo": "owner/repo", "search": "is:open label:bug", "events": ["opened"] },
      "created_by": "user" },
    { "id": "n_7hx2pm", "kind": "agent", "title": "Triage", "x": 420, "y": 0, "w": 440, "h": 560, "z": 2,
      "config": { "model": "", "effort": "high", "sandbox": "off", "output": "final",
                  "instructions": "Classify each issue; for real bugs spawn a Codex worker in a worktree." },
      "access": "build", "created_by": "user",
      "runtime": { "session": "0c1202c5-fcf0-487b-b937-9824cfe3b298" } },
    { "id": "n_p0r4cz", "kind": "terminal", "preset": "claude-code", "title": "coder",
      "x": 940, "y": 120, "w": 760, "h": 520, "z": 3,
      "config": { "worktree": { "name": "fix-42" }, "permission_mode": "acceptEdits", "input": "shared" },
      "access": "talk", "created_by": "n_7hx2pm",
      "runtime": { "terminal": "9f2c…", "agent_session": "3e0d…-uuid", "worktree": "/…/.harness/worktrees/fix-42",
                   "branch": "kou/fix-42", "epoch": 1 } }
  ],
  "edges": [
    { "id": "e_q81", "from": { "node": "n_k3d9qa", "port": "opened" }, "to": { "node": "n_7hx2pm", "port": "in" },
      "template": "New issue #{{data.number}}: {{data.title}}\n{{data.url}}\n\n{{data.body}}",
      "mode": "auto", "deliver": "queue", "header": true }
  ]
}
```

### 4.2 Поля и инварианты

- **ID нод и рёбер** — `n_` / `e_` плюс 6 символов base32, уникальны внутри канваса.
- **`kind`** — встроенные `terminal`, `agent`, `source`, `note`. **`preset`** уточняет вид: `shell`, `command`, `claude-code`, `<plugin>:<id>`.
- **`config`** хранится как `jsontext.Value`, то есть без потерь. Валидируется по схеме вида или пресета на сервере. Незнакомые поля сохраняются.
- **`runtime`** пишет только сервер. Поля: `terminal`, `session`, `agent_session` (ID сессии Claude/Codex — для resume), `worktree`, `branch`, `epoch` (для отзыва токенов). Клиенты его только читают.
- **`created_by`**: `"user"` или ID ноды-агента. По нему проверяется право удалять (§9.1).
- **Координаты** — мировые пиксели при масштабе 1. Вьюпорт (pan/zoom) у каждого браузера свой (`prefs`), в документ не пишется.
- **`version`** для совместимости вперёд. Документ новее, чем знает сервер, открывается только на чтение с предупреждением.

### 4.3 Где хранится

| Что | Путь | Примечание |
|---|---|---|
| Документ | `<ws>/.harness/canvases/<id>.json` | Атомарно: temp + rename, как `workspaces.save` |
| Журнал сообщений | `<ws>/.harness/canvases/<id>/messages.jsonl` | Ротация при 5 MiB → `.1` |
| Очередь доставки | `<ws>/.harness/canvases/<id>/pending.jsonl` | Переживает рестарт и пересборку (§5.6) |
| Большие тексты сообщений | `<ws>/.harness/canvases/<id>/messages/<msg>.txt` | §8.7 |
| Состояние источников | `<ws>/.harness/canvases/<id>/state/<node>/` | Курсоры и дедуп-ключи (§10.2) |
| Файлы запуска пресетов | `<user cache>/kou-conveyor/canvas/<ws-id>/<canvas>/<node>/` | settings.json для Claude, mcp.json — ничего секретного |
| Каталог CLI для PATH | `<user cache>/kou-conveyor/canvas-bin/` | Только симлинк `kou-canvas` (§9.3) |
| Секрет токенов | `<config dir>/canvas.secret` (0600) | §9.1 |

`.harness/` уже содержит `sessions/` и `worktrees/` и в этом репозитории в `.gitignore` — канвасы следуют тому же.

### 4.4 Операции и ревизии

Все изменения документа — пакеты операций. Их применяет только сервер, по одному за раз (single writer).

```jsonc
POST /api/w/{ws}/canvases/{id}/ops
{ "base_rev": 41, "ops": [
  { "op": "node.add",    "node": { "kind": "terminal", "preset": "codex", "title": "worker-a", "near": "n_7hx2pm", "side": "right" } },
  { "op": "node.update", "id": "n_p0r4cz", "set": { "x": 980, "y": 140 } },
  { "op": "node.remove", "id": "n_old123", "keep": { "worktree": true, "session": true }, "grace_s": 15 },
  { "op": "edge.add",    "edge": { "from": { "node": "n_7hx2pm", "port": "out" }, "to": { "node": "n_p0r4cz", "port": "in" } } },
  { "op": "edge.update", "id": "e_q81", "set": { "mode": "approve" } },
  { "op": "edge.remove", "id": "e_q81" },
  { "op": "canvas.update", "set": { "title": "Issue triage", "live": true } }
] }
→ 200 { "rev": 42, "ids": { "0": "n_m2c8x1", "3": "e_v7d" }, "placed": { "n_m2c8x1": { "x": 940, "y": 0, "w": 760, "h": 520 } } }
```

- Пакет атомарен: применяется целиком или отклоняется с ошибкой валидации.
- `base_rev` необязателен. Геометрия и свойства — last-writer-wins. Структурный конфликт (ребро к удалённой ноде) → `409` с текущим `rev`, клиент перечитывает снимок.
- Побочные эффекты — запуск терминала новой ноды, остановка удалённой, создание worktree — выполняются после коммита. Их результат приходит отдельным событием `runtime`.
- Каждый пакет транслируется в SSE `{"type":"ops","rev":42,"actor":{"kind":"user"|"node","id":…},"ops":[…]}`.
- Undo/redo — на клиенте: стек обратных операций только для своих пакетов. Удаление терминальной ноды обратимо в течение grace: ядро держит PTY через `CloseAfter`, обратная `node.add` с тем же `runtime.terminal` вызывает `Keep`.
- Документ пишется на диск с дебаунсом 250 мс. При shutdown и перед `exec` пересборки — сразу.

### 4.5 Сообщение

```jsonc
{ "id": "m_…", "chain": "m_root…", "hops": 3,
  "from": { "node": "n_k3d9qa", "port": "opened" }, "edge": "e_q81", "to": { "node": "n_7hx2pm", "port": "in" },
  "at": "…", "title": "#42 Crash on save", "text": "…", "data": { "number": 42, "url": "…" },
  "file": "",            // путь к полному тексту, если обрезан
  "state": "delivered"   // pending | awaiting_approval | delivered | dropped
}
```

Имя `Inbox` в харнесе уже занято (дедупликация входа сессии, README → Glossary), поэтому у канваса «очередь доставки» (`pending`).

---

## 5. Сервер: движок канваса

### 5.1 Пакеты и файлы

```
cmd/internal/canvas/            новый пакет, без зависимостей от web-сервера
  doc.go        Doc/Node/Edge/Port/Message, Validate, ID-генераторы, version
  store.go      Load/Save/List/Delete (атомарно), миграции version
  ops.go        Apply(doc, batch) → (doc', effects, error); инварианты; лимиты
  hub.go        живое состояние одного канваса: подписчики SSE, статусы, дебаунс записи
  engine.go     Engine: канвасы по воркспейсам, загрузка live при старте, Busy/Flush
  router.go     маршрутизация: fan-out/in, шаблоны, режимы, pending, защита от циклов, журнал
  template.go   {{text}}, {{data.a.b}}, {{from.title}} …
  place.go      поиск свободного места рядом с якорем (§9.6)
  token.go      HMAC-токены ноды и скоупы (§9.1)
  kinds.go      реестр видов: встроенные и пришедшие из плагинов
  terminal.go   вид terminal: запуск/привязка PTY, доставка, OSC 133, idle, пресеты
  agent.go      вид agent (kou): сессия ноды, доставка через Host.Enqueue, хуки ранов
  source.go     вид source: супервизор процессов-источников (stream/poll)
  preset.go     пресеты харнесов: подстановки, файлы запуска, brief
cmd/internal/worktree/          вынесенный из agentrunner/sandbox.go ensureWorktree (§6.5)
cmd/kou-conveyor-web/canvas.go  HTTP-обработчики, реализация Host, проводка в server
cmd/kou-conveyor-canvas/        CLI и MCP-сервер (§9.3, §9.5)
```

### 5.2 Ключевые интерфейсы (эскиз)

```go
// Host — то, что движку даёт web-сервер; канвас не знает о server напрямую.
type Host interface {
	StartTerminal(spec terminal.Spec) (*terminal.Session, error)
	Terminal(id string) (*terminal.Session, error)
	CloseTerminal(id string, grace time.Duration) error
	Enqueue(ws, sessionID, text string, force bool) (EnqueueResult, error) // ядро handleEnqueue
	StopRun(ws, sessionID string) error
	Plugins(ws string) plugin.Found
	URL() string // http://127.0.0.1:<port>, для KOU_CANVAS_URL
}

// Kind — вид ноды на сервере.
type Kind interface {
	Name() string
	Validate(n *Node) error
	Ports(n *Node) Ports
	Start(ctx context.Context, hub *Hub, n *Node) (Instance, error)
}

// Instance — работающая нода.
type Instance interface {
	Deliver(m Message, how Delivery) error
	Status() Status
	Read(what string, lines int) (string, error) // screen | tail | output | answer
	Close(grace time.Duration) error
}

// Engine — один на сервер.
func (e *Engine) RunEnv(ws, sessionID string) []string                        // env канвас-ранов kou
func (e *Engine) RunStarted(ws, sessionID string, events RunEvents)            // подписка на события рана
func (e *Engine) RunFinished(ws, sessionID, answer, outcome string)            // выход kou-ноды
func (e *Engine) Busy() bool
func (e *Engine) Flush()
```

### 5.3 HTTP API

Маршруты UI регистрируются в существующем цикле `for _, prefix := range []string{"/api/w/{ws}", "/api"}` в `server.go`. Агентские маршруты `/api/canvas/*` требуют токен: воркспейс, канвас и нода определяются по нему.

| Метод | Путь | Кто | Что |
|---|---|---|---|
| GET | `{prefix}/canvases` | UI | список: id, title, updated_at, live, nodes, agents (running/waiting) |
| POST | `{prefix}/canvases` | UI | создать `{id?, title?, template?}` |
| GET | `{prefix}/canvases/{id}` | UI | документ, статусы, runtime |
| PATCH | `{prefix}/canvases/{id}` | UI | `title`, `live`, `settings` |
| DELETE | `{prefix}/canvases/{id}` | UI | удалить; параметры: `end_terminals`, `keep_sessions`, `keep_worktrees` |
| POST | `{prefix}/canvases/{id}/ops` | UI | пакет операций (§4.4) |
| GET | `{prefix}/canvases/{id}/events` | UI | SSE (§5.4), возобновление по `Last-Event-ID` |
| POST | `{prefix}/canvases/{id}/nodes/{node}/send` | UI | `{text, submit, keys, deliver}` — ввод, как от человека |
| GET | `{prefix}/canvases/{id}/nodes/{node}/read` | UI | `?what=screen\|tail\|output\|answer&lines=` |
| GET | `{prefix}/canvases/{id}/nodes/{node}/transcript` | UI | компактный хвост kou-сессии `?tail=40` |
| POST | `{prefix}/canvases/{id}/nodes/{node}/restart` | UI | перезапустить процесс ноды; `{resume: true}` для харнесов |
| POST | `{prefix}/canvases/{id}/messages/{msg}/approve` | UI | пропустить сообщение approve-ребра (можно с правкой текста) |
| DELETE | `{prefix}/canvases/{id}/messages/{msg}` | UI | отбросить pending-сообщение |
| GET | `{prefix}/canvas/kinds` | UI, агент | виды, пресеты харнесов, источники плагинов, JSON-схемы |
| GET | `/api/canvas/self` | агент | канвас, нода, скоупы, brief, URL канваса |
| GET | `/api/canvas/self/launch` | CLI `launch` | argv, env и файлы запуска пресета ноды (§6.2) |
| GET | `/api/canvas/view` | агент | граф с координатами и подсказками свободного места (§9.2) |
| POST | `/api/canvas/ops` | агент (`build`) | те же операции; actor = нода токена |
| POST | `/api/canvas/nodes/{node}/send` | агент (`talk`) | как UI-send, с проверкой скоупа |
| POST | `/api/canvas/emit` | хук, источник, агент (`talk`) | `{port, text, data, key, status}` — выход своей ноды |
| GET | `/api/canvas/nodes/{node}/read` | агент (`observe`) | чтение |
| GET | `/api/canvas/nodes/{node}/wait` | агент (`observe`) | long-poll: `?until=idle\|output\|exit&timeout=600` |
| POST | `/api/canvas/hooks/{hook}` | локальный клиент | webhook-источник (фаза 5) |

### 5.4 Поток событий (SSE)

Один поток на открытую страницу канваса. Отдельные SSE на каждый ран kou-ноды не открываем: у браузера всего 6 HTTP/1.1-соединений на хост, а сервер на localhost работает без HTTP/2.

| `type` | Поля | Для чего |
|---|---|---|
| `snapshot` | `doc`, `status{node→…}`, `pending{node→n}`, `rev` | первый кадр и пересинхронизация |
| `ops` | `rev`, `actor`, `ops[]` | изменения документа |
| `runtime` | `node`, `runtime` | терминал запущен, сессия создана, worktree готов |
| `status` | `node`, `status`, `detail`, `since`, `activity` | статусы (§3.4) |
| `message` | `id`, `edge`, `from`, `to`, `state`, `preview` (≤ 300 симв.), `reason` | анимация рёбер, бейджи pending, Activity |
| `agent` | `node`, `entry` (компактная: kind, phase, текст ≤ 2 KiB, tool{name, state, summary}) | живая лента kou-нод |
| `terminal` | `node`, `title`, `cwd`, `running` | подвал терминальной ноды |
| `notice` | `level`, `text`, `node?`, `edge?` | «защита от циклов остановила цепочку…», «источник падает» |

Терминалы по-прежнему подключаются собственными WebSocket (`/api/terminals/{id}/socket`). Их пул отдельный от HTTP/1.1-соединений, и это существующий протокол без изменений.

### 5.5 Связь с ранами и очередью

Три аддитивных вызова в `runs.go` и вынос ядра очереди:

1. В `launch` перед `cockpit.Start`: `env := s.canvas.RunEnv(ws, req.SessionID)` → `cockpit.Request.Env` (новое внутреннее поле, в JSON раннера не уходит). Канвас-контекст берётся **из сессии**, а не из запроса. Поэтому любой ран kou-ноды получает одни и те же env и инструменты: из ноды, из полной сессии, из очереди (`runNext`), правкой промпта.
2. После регистрации рана (`s.runs[current.id] = current`): `s.canvas.RunStarted(ws, sessionID, current)`. Движок читает `current.since()` так же, как `handleEvents`, только внутри процесса, и шлёт компактные `agent`/`status` в SSE канваса.
3. В `pump` после `current.publish(done)`: `s.canvas.RunFinished(ws, sessionID, finalAnswer(tr), outcome)`. `finalAnswer` — последняя запись `assistant` с `Phase=final_answer`, иначе последний текст ассистента после последнего промпта.
4. Из `handleEnqueue` выносится `func (s *server) enqueue(ctx, ws, id string, item enqueueItem) (…)`. Обработчик становится тонкой обёрткой с прежним поведением; это покрыто `queue_test.go`. Движок вызывает тот же метод через `Host.Enqueue`.

Если движок выключен (`-canvas=off`), `s.canvas` — no-op-реализация, и все три вызова ничего не делают.

### 5.6 Жизненный цикл

- **Старт сервера.** `s.startCanvas()` вызывается после `startTerminals()`, когда терминалы, переданные прошлой сборкой, уже усыновлены. Порядок:
  1. загрузить документы с `live: true` во всех воркспейсах списка;
  2. привязать терминальные ноды к их `runtime.terminal`; если такого PTY нет — статус `stopped` с кнопками Start/Resume;
  3. поднять источники;
  4. восстановить `pending.jsonl`.
- **Самопересборка** (`rebuild.go`). Перед `execSelf` выполняется `s.canvas.Flush()`: документы, pending и журнал пишутся на диск. Источники при `exec` теряют пайпы и завершаются; новая сборка их перезапускает, а курсоры и дедуп-ключи (§10.2) не дают задвоить события. `busy()` канвас не блокирует: pending лежит на диске. Если наблюдение покажет, что пересборка посреди доставки мешает, добавим `s.canvas.Busy()` для «доставка в процессе» (секунды).
- **Shutdown** (`close`). Flush, затем SIGTERM источникам. Терминалы гасит существующий `s.terminals.Close()`.
- **Удаление воркспейса из списка.** `409`, если в нём есть live-канвасы («поставьте их на паузу»), по аналогии с идущими ранами.
- **Handover терминалов.** В записи `handedOver` добавляются `Owner` и `Env` (§6.1), чтобы после пересборки PTY оставались «канвасными».

---

## 6. Терминальные ноды

### 6.1 Изменения пакета `cmd/internal/terminal` (аддитивно)

```go
type Spec struct {
	Workspace, Dir, Shell string
	Theme                 bool
	Cols, Rows            int
	// новое:
	Owner string   // "canvas:<ws>/<canvas>/<node>"; у таких PTY свой хозяин, сайдбар их не показывает
	Env   []string // KOU_CANVAS_URL/TOKEN/ID/NODE, KOU_CANVAS_BIN, KOU_CANVAS_CLI
}

type Info struct { /* … */ Owner string `json:"owner,omitzero"` }

func (m *Manager) List(workspace string) []Info        // как было, но без PTY с Owner → сайдбар не меняется
func (m *Manager) ListOwned(owner string) []Info       // для канваса

func (s *Session) Paste(text string, o PasteOptions) error // §6.3
func (s *Session) Keys(names ...string) error              // §6.3
func (s *Session) Observe(fn func(Event)) (cancel func())  // вывод, метки OSC 133, meta, exit — без учёта в Clients
func (s *Session) Tail(lines int) string                   // хвост кольца без ANSI (charmbracelet/x/ansi уже в go.mod)
func (s *Session) Screen() (string, bool)                  // фаза 6 / спайк 0.4: текст экрана от серверного эмулятора
```

- **Сканер** (`scan.go`):
  - разбирать **OSC 133** (A — приглашение, B — начало ввода, C — команда пошла, D;код — команда закончилась) и сообщать метки в `Observe`;
  - отслеживать DECCKM (режим 1) — от него зависят коды стрелок в `Keys`.
- **`Observe`** вызывается из `pump` под локом сессии и обязан не блокировать. Поэтому у движка свой буфер: кольцо и сигнал, без канала на 1024 сообщения, как у клиентов.
- **Лимиты.** `maxSessions` (64) — общий потолок. Канвас добавляет свой потолок на канвас (`max_terminals`, по умолчанию 16). Поднимать общий — по итогам спайка 0.1.
- **Интеграция шеллов.** В `kou.bash` добавить 133;C (через `PS0`) и 133;D;код (в `PROMPT_COMMAND`), в `kou.fish` — через `fish_preexec`/`fish_postexec`. Для zsh всё уже есть. Плюс: если задан `KOU_CANVAS_BIN`, добавить его в начало `PATH` **после** файлов пользователя (§9.3).

### 6.2 Запуск

| Режим | Что происходит | Для чего |
|---|---|---|
| `shell` | логин-шелл как сейчас (с интеграцией, если тема kou) | пресет `shell` |
| `type` | логин-шелл; после первого приглашения (133;A, без интеграции — через 1.5 с) набирается `command\r` | пресет `command`: команда видна и остаётся в истории, после выхода — шелл |
| `launcher` | логин-шелл; после приглашения набирается ` kou-canvas launch\r` (с пробелом в начале — не в историю). CLI берёт у сервера argv, env и файлы пресета (`/api/canvas/self/launch`) и делает `exec` харнеса | пресеты харнесов: короткая строка вместо километра флагов; после выхода из харнеса — снова шелл; `kou-canvas launch --resume` — продолжить сессию |

Логин-шелл с интеграцией загружает пользовательские `PATH`, алиасы и nvm. Так `claude`, установленный через npm в `.zshrc`, находится так же, как в обычном терминале. Поэтому харнес не запускаем напрямую из Go.

### 6.3 Ввод «как tmux»

- **`Paste(text, {submit, newline, delay})`:**
  1. **Очистка** — защита от инъекций. Удаляются ESC (0x1B), C1 (U+0080–U+009F) и прочие C0, кроме `\t` и `\n`. В частности, текст не может «закрыть» вставку последовательностью `ESC[201~` и выполнить остаток как набранные клавиши.
  2. Если программа включила режим 2004: `ESC[200~` + текст + `ESC[201~`. Иначе — посимвольно одной записью. В шелле многострочный текст тогда выполняется построчно, как при вставке в обычный терминал.
  3. `\n` → `\r`, как делают терминалы при вставке (настраивается пресетом).
  4. При `submit` — `\r` через `submit_delay_ms` (по умолчанию 60 мс; TUI-шки различают «вставку» и «Enter» по времени — подбирается в спайке 0.2).
- **`Keys(...)`** — именованные клавиши, как у tmux: `Enter`, `Tab`, `Escape`, `BSpace`, `Up/Down/Left/Right` (`ESC[A` или `ESCOA` по DECCKM), `Home/End`, `PageUp/PageDown`, `C-a…C-z`, `M-x`, `F1…F12`. Для агентов это способ подтвердить диалог разрешений, прервать (`Escape`, `C-c`), выбрать пункт меню.
- **Готовность.** Доставка по рёбрам по умолчанию ждёт `idle` цели: приглашение шелла по OSC 133, хук `Stop` у харнеса, простой у generic TUI. Режим `now` пишет сразу.
- **Атомарность.** Одна доставка — одна запись под `s.writing`. Это существующий мьютекс, поэтому символы человека и агента не перемешиваются внутри одной вставки.

### 6.4 Выход и статус

| Источник | Статус | Выход (`out`) |
|---|---|---|
| OSC 133 (шелл с интеграцией) | A/B → `idle`, C → `busy`, D → `idle` и код | текст между C и D без ANSI, ≤ 256 KiB; в `data`: `exit_code`, `command` (строка после B, если удалось вычленить) |
| Хуки харнеса (Claude Code) | `UserPromptSubmit` → `busy`, `Stop` → `idle`, `Notification` → `waiting` | последний ответ ассистента (§7.3) |
| `notify` (Codex) | доставка → `busy`, `agent-turn-complete` → `idle` | `last-assistant-message` (§7.4) |
| Простой (generic TUI) | вывод был < `idle_ms` назад → `busy`, иначе `idle`; опционально `ready`-regex по последней строке экрана | по умолчанию нет (нода — «приёмник»); опционально — новый текст экрана с момента доставки |
| Выход процесса | `exited` + код | порт `exit`: `{code}` |

### 6.5 Worktrees

- `ensureWorktree` выносится из `cmd/internal/agentrunner/sandbox.go` в `cmd/internal/worktree`: `Ensure(ctx, workspace, name, base string) (path, branch string, err error)`. Соглашения раннера сохраняются: `.harness/worktrees/<name>`, ветка `kou/<name>`, `git worktree prune` перед созданием, повторное использование существующих. Раннер вызывает пакет, его тесты не меняются.
- Терминальная нода получает `config.worktree: { name?, branch?, base? }`. Без `name` имя берётся из названия ноды (slug) или ID. `Dir` терминала = путь worktree. В подвале ноды видны ветка и путь.
- kou-нода получает `config.sandbox: "worktree"`. Это env `KOU_CONVEYOR_SANDBOX=worktree` для её ранов, и раннер сам создаёт `.harness/worktrees/<session8>` на ветке `kou/<session8>`: существующий механизм, ноль нового кода в раннере.
- **Никаких автоудалений.** Удаление ноды оставляет worktree. Пункт меню «Remove worktree» делает `git status --porcelain`: если копия грязная — отказ с объяснением, если чистая — подтверждение, затем `git worktree remove`. Ветку не трогаем.
- Если запросить два «пишущих» узла в одном worktree, UI предупредит («один писатель на рабочую копию»), но не запретит.
- v2: kou-нода «в worktree другой ноды», например ревьюер внутри worktree кодера. Нужен `cockpit.Options.Workspace` для отдельного рана и трекер изменений на каталог. В v1 ревьюер читает файлы по абсолютному пути из `CanvasView`.

### 6.6 Рендер в браузере

- Плагин `terminal` предоставляет сервис `terminal`, вынеся из `view.js` транспорт одной панели и инициализацию restty:
  ```js
  cockpit.provide('terminal', {
    mount(container, { id, cols, rows, fontSize, readOnly, onMeta, onExit, onFocus }) // → { focus, blur, resize, dispose }
  });
  ```
  Вкладки сайдбара продолжают работать на том же коде, это чистый рефакторинг. Если плагин `terminal` выключен, канвас показывает текстовые снимки (`read?what=screen|tail`) и поле «Send».
- **LOD** (уровень детализации), пороги уточняет спайк 0.1:
  - «живой» restty только у нод в вьюпорте при масштабе ≥ 0.6 и не более 8 одновременно (LRU);
  - остальные показывают снимок экрана — `<pre>`, обновление по активности не чаще 1/с;
  - при масштабе < 0.35 видны только рамка, заголовок и статус.
- **Масштаб и restty.** restty меряет контейнер через `getBoundingClientRect`, поэтому внутри `transform: scale()` он посчитает «ужатую» сетку и сделает resize PTY. План:
  - `autoResize: false` и фиксированная сетка cols×rows ноды; ресайз только при изменении размера ноды;
  - при масштабе ≠ 1 мышь и выделение в терминале выключены (`pointer-events: none`), работает клавиатура;
  - двойной клик или Enter — «режим фокуса»: зум на 100 % по центру, полноценная мышь.

  Если спайк покажет, что `autoResize: false` при масштабе ведёт себя плохо: живой терминал рендерится вне трансформируемого слоя и позиционируется поверх ноды в экранных координатах, при масштабе ≠ 1 — снимок.

---

## 7. Агентные ноды (харнесы)

### 7.1 kou-агент — «упрощённая сессия нашего харнеса»

**Сервер:**

- `node.add(kind=agent)`: генерируется UUID сессии и пишется `.meta/<id>.json` с `Title` = название ноды, `Canvas`, `Node`. Файл сессии создаёт раннер на первом ране — как всегда.
- **Конфиг:**

  | Поле | Значения |
  |---|---|
  | `model` | модель |
  | `effort` | уровень усилия |
  | `sandbox` | `off` \| `worktree` |
  | `output` | `final` \| `explicit` \| `both` — авто-выход финальным ответом или только явный `CanvasEmit` |
  | `instructions` | роль ноды |
  | `access` | `none` \| `talk` \| `build` \| `admin` (§9.1) |
  | `tool_profile` | профиль инструментов, опционально |

- **Доставка:** `Host.Enqueue(ws, session, text, force)`. Это ровно семантика очереди: при простое ран стартует, при ране сообщение ждёт, при `deliver: now` — steer.
- **Статус** берётся из рана (`activity` транскрипта), очереди (`pending`) и исхода (`failed` → `error`).
- **Выход:** `RunFinished` эмитит в `out` финальный ответ, если `output` включает `final`.
- **Env ранов** (`RunEnv`): `KOU_CANVAS_*`, `KOU_CANVAS_BIN` в начало `PATH`, `KOU_CONVEYOR_SANDBOX=worktree` при необходимости. Раннер видит `KOU_CANVAS_TOKEN`, поэтому плагин `canvas-agent` у него активен (§9.4).
- **Роль (brief, §7.6).** v1 — префиксом первого сообщения ноды. Пользовательские промпты переживают компакцию дословно (до 40 000 символов, `cmd/kou-conveyor-runner/README.md` → Compaction), так что роль не теряется. v2 (опционально) — поле запроса раннера `append_system_prompt` с определением по `-h`, как `steer`/`links` в `featuresOf`. Существующее `system_prompt` не подходит: оно **заменяет** системный промпт вместе с рабочими нормами.

**Веб:**

- **Компактная лента:**
  - начальный хвост — из `/nodes/{node}/transcript?tail=40`, дальше живые `agent`-события из SSE канваса;
  - промпты — одной строкой, с чипом происхождения «from Triage»;
  - ответы — через сервис `markdown`;
  - вызовы инструментов — строкой «▸ Bash · go test ./… · ✓ 12s»;
  - уведомления и ошибки.
- **Инпут.** Textarea со ссылками `$` через `files.attach({ input, container })` — существующий сервис. Enter — отправить (при простое — ран, при работе — в очередь), ⌘Enter — force (steer), Esc Esc — стоп. Чипы модели (`models.catalog()`) и усилия.
- **Меню ⋯:** Open session (`#/w/<ws>/s/<sid>` — вся мощь основного таймлайна: правки, ветки, изменения), Stop, Compact, Copy last answer, Change model, Delete node (сессия остаётся) / Delete node and session.

Почему это «упрощённая» сессия и при этом не форк кода: модель сессии (`session` plugin) рассчитана на один `view()` за раз, поэтому каждая нода держит собственное лёгкое состояние. Тяжёлые сценарии (правка промпта, ветки, diff) открываются в полном виде одним кликом.

### 7.2 Пресеты харнесов — данные, а не код

Пресет — элемент `canvas.harnesses` в `plugin.json` (§10.1). Встроенные пресеты лежат в `plugin.json` встроенного плагина `canvas`. Плагин пользователя или воркспейса может добавить свой пресет или заменить встроенный по `id`.

| Поле | Значение |
|---|---|
| `id`, `title`, `icon`, `description` | идентификация и вид в меню Add |
| `command` | argv0 и фиксированные аргументы; ищется в PATH шелла ноды |
| `args` | аргументы с подстановками `{{brief}}`, `{{files.<name>}}`, `{{node.title}}`, `{{node.id}}`, `{{runtime.agent_session}}`, `{{config.<key>}}` |
| `files` | `{ name: json \| text }` — пишутся в каталог запуска ноды перед стартом (§4.3) |
| `env` | дополнительные переменные |
| `launch` | `launcher` (по умолчанию для харнесов) \| `type` \| `shell` |
| `input` | `{ paste: "bracketed" \| "type", newline: "cr" \| "lf", submit: "\r", submit_delay_ms }` |
| `status` | список: `hooks`, `notify`, `osc133`, `idle`; `idle_ms`, `ready` (regex) |
| `output` | `hooks` \| `notify` \| `osc133` \| `screen` \| `none` |
| `session_arg` | как задать ID сессии харнеса при старте (`["--session-id", "{{runtime.agent_session}}"]`) |
| `resume` | аргументы продолжения (`["--resume", "{{runtime.agent_session}}"]`) |
| `config` | JSON-схема настроек ноды (permission mode и т. п.) → форма инспектора |
| `check` | команда проверки наличия и версии (`["claude", "--version"]`), `min_version` |

### 7.3 Claude Code

Флаги ниже проверены по `claude --help` версии 2.1.287 на этой машине. Поведение хуков — **проверить в спайке 0.3**.

- **Запуск:**
  ```
  claude --session-id <uuid ноды> --settings <dir>/claude-settings.json --mcp-config <dir>/mcp.json \
         --append-system-prompt "<brief>" [--permission-mode <config.permission_mode>]
  ```
  `--session-id` задаём сами, поэтому знаем ID для `--resume` после краха или рестарта без handover.
- **`claude-settings.json`** (передаётся флагом, файлы пользователя не трогаются) — хуки `SessionStart`, `UserPromptSubmit`, `Stop`, `Notification`; каждый — `{"type":"command","command":"\"${KOU_CANVAS_CLI:-kou-canvas}\" hook claude","timeout":5}`.
- **`kou-canvas hook claude`** читает JSON со stdin (`session_id`, `transcript_path`, `hook_event_name`, …) и отправляет `POST /api/canvas/emit`:
  - `UserPromptSubmit` → статус `busy`;
  - `Stop` → статус `idle`, `out` = последнее сообщение ассистента из `transcript_path` (JSONL) или из поля события, если версия его даёт;
  - `Notification` → статус `waiting` + текст («Claude needs your permission to use Bash»);
  - `SessionStart` → `idle`, записывает `runtime.agent_session`.

  Без `KOU_CANVAS_TOKEN` хук мгновенно выходит с кодом 0 — он **инертен** вне канваса (так же сделаны хуки herdr в `~/.codex`). Хук никогда не блокирует Claude: короткий таймаут, ошибки глотаются, код выхода 0.
- **`mcp.json`**: `{"mcpServers":{"kou-canvas":{"command":"kou-canvas","args":["mcp"]}}}`; env (`KOU_CANVAS_*`) наследуется от шелла ноды.
- **Ввод:** bracketed paste и `\r` через ~80 мс (подобрать). Длинные вставки Claude сворачивает в «[Pasted text #1]» — это нормально.

### 7.4 Codex

`codex --help` версии 0.160.0: `-c key=value` (TOML-оверрайды), `--cd`, `--sandbox`, `--ask-for-approval`, подкоманды `resume`, `queue`, `mcp`; фича `hooks` — stable. **Проверить в спайке 0.3.**

- **Запуск:**
  ```
  codex -c 'notify=["kou-canvas","hook","codex"]' \
        -c 'mcp_servers.kou-canvas.command="kou-canvas"' -c 'mcp_servers.kou-canvas.args=["mcp"]' \
        [-c 'mcp_servers.kou-canvas.env_vars=[…]']   # проброс KOU_CANVAS_* в MCP-процесс
        [-C <worktree>] [--sandbox …] [--ask-for-approval …]
  ```
  Нужно проверить, наследует ли stdio-MCP Codex переменные окружения. Если нет — пробросить через конфиг MCP-сервера. Токен не кладём в argv: он виден в `ps`.
- **`notify`.** Codex вызывает программу с JSON последним аргументом. `kou-canvas hook codex '<json>'` при `type=agent-turn-complete` отправляет `out` = `last-assistant-message`, статус `idle` и запоминает `thread-id` → `runtime.agent_session` (для `codex resume <id>`).
- **Статус `busy`** выставляется при доставке ввода. Если хуки Codex дают аналог `UserPromptSubmit` — подключить их тем же способом.
- **Ввод:** bracketed paste + Enter. Альтернатива — структурный ввод `codex queue --thread <id> --message <text>` (есть в 0.160). Оценить в спайке: если работает с идущей TUI-сессией, это надёжнее эмуляции клавиш.

### 7.5 Прочие TUI (OpenCode, aider, gemini…)

- Generic-пресет: `status: ["idle"]`, `idle_ms: 4000`, `output: "none"`. По умолчанию нода — «приёмник»: принимает ввод по рёбрам, но сама не эмитит. Включить `output: "screen"` можно на свой риск: эмитится новый текст экрана после доставки.
- Глубокие интеграции — отдельными плагинами-пресетами. У OpenCode есть `serve`, HTTP API и плагины, так что адаптер на его событиях возможен; у aider — флаги и файловые чаты.

### 7.6 Brief — что агент знает о себе

Сервер генерирует brief из канваса и конфига ноды, ≤ 1.5 KiB:

```
You are node «coder» (n_p0r4cz, Claude Code) on the kou-conveyor canvas «Issue triage».
Messages from other nodes arrive as prompts that start with [canvas] from «…».
Your answer at the end of each turn goes to: Triage (◆ kou agent).
You work in the git worktree /…/.harness/worktrees/fix-42 on branch kou/fix-42; commit there.
The canvas changes while you work: run `kou-canvas view` (or the kou-canvas MCP tools) to see the
current nodes, their positions and connections before you create or connect anything.
Your access: talk — you can read and message nodes, but not create or delete them.
```

- Claude Code получает brief через `--append-system-prompt`.
- Codex — через developer instructions, если `-c` это позволяет (**проверить**), иначе префиксом первого сообщения.
- kou — префиксом первого сообщения (§7.1).
- Связи могут поменяться после запуска, поэтому brief говорит «смотри `view`», а не перечисляет связи как истину.

---

## 8. Связи и маршрутизация

### 8.1 Путь сообщения

```
нода эмитит на порт P ──► для каждого включённого ребра из (N, P):
   шаблон ребра → заголовок происхождения → размер (§8.7)
   → режим ребра: off ✗ | approve → pending(awaiting) | auto ↓
   → защита от циклов и лимиты (§8.5) ✗ notice
   → очередь доставки цели (pending, FIFO, на диске)
   → когда цель готова (deliver=queue: idle; now: сразу) → Instance.Deliver
   → журнал + SSE message
```

### 8.2 Доставка по видам цели

| Цель | Как |
|---|---|
| терминал (шелл) | `Paste(text, submit=true)`: текст — команда |
| терминал (харнес) | `Paste` по `input` пресета (bracketed + Enter с задержкой) |
| kou-агент | `Host.Enqueue(…, force = deliver=="now")` |
| источник | не принимает (у источника нет входов). Управляющий вход «poll now» — позже |
| заметка | нет портов |

### 8.3 Шаблоны

Минималистичный синтаксис без логики:

- `{{text}}`, `{{title}}`, `{{data.<путь>}}`;
- `{{from.title}}`, `{{from.id}}`, `{{from.kind}}`;
- `{{edge.id}}`, `{{now}}`.

Отсутствующее поле подставляется пустой строкой, предпросмотр в инспекторе подсвечивает отсутствующие поля. По умолчанию:

- для целей-агентов: заголовок `[canvas] from «{{from.title}}»:` + `{{text}}` (выключается флагом `header`);
- для шелла: `{{text}}`.

### 8.4 Режимы ребра и fan-in/out

- `mode`: `auto` | `approve` | `off`. `deliver`: `queue` (ждать готовности цели — по умолчанию) | `now`.
- **Fan-out**: каждое ребро из порта получает копию. **Fan-in**: сообщения встают в очередь цели по времени прихода.
- Позже (фаза 6): `filter` — простое условие по `data` (`data.labels contains "bug"`), без исполнения кода.

### 8.5 Защита от циклов и лимиты

- **Цепочки и шаги.** У сообщения есть `chain` (ID корня) и `hops`. Выход ноды, вызванный доставкой, наследует цепочку и `hops + 1`. Связь определяется так: ран kou или ход харнеса начался из-за этой доставки. При `hops > max_hops` (по умолчанию 32) сообщение отбрасывается, ребро подсвечивается, в Activity появляется notice.
- **Циклы агент→агент.** Ребро, замыкающее цикл между агентами, при создании получает `mode: approve` (это видно и меняется). Цикл «агент ↔ шелл» (REPL) — тоже цикл. Для него лучше инструменты (`send`/`read`/`wait`), а не рёбра; так и советуют подсказка в UI и skill агента.
- **Скорость.** Ребро — `edge_rate_per_minute` (30), канвас — `canvas_rate_per_minute` (120). Сверх лимита сообщения не теряются, а копятся в pending с пометкой «rate-limited».
- **Предохранитель.** 3 неудачные доставки подряд (ошибка Enqueue, мёртвый PTY) → ребро в `off`, notice.
- **Глубина спавна.** Агент, созданный агентом, имеет глубину +1; `max_depth` по умолчанию 3 (§9.8).

### 8.6 Live / Paused

- **Live** — источники работают, маршрутизация включена.
- **Paused** — источники остановлены, сообщения копятся в pending. Терминалы и агенты живут, человек с ними работает.
- Состояние сохраняется (`live` в документе). После рестарта сервера live-канвасы продолжают работу.
- В подвале рейла (слот `rail.foot`) — бейдж «◧ 2 live», чтобы фоновая активность не была сюрпризом.
- Глобальная кнопка «Pause all canvases» — в палитре и в меню бейджа.

### 8.7 Большие сообщения

Текст ≤ 32 KiB уходит как есть. Больше — полный текст пишется в `.harness/canvases/<id>/messages/<msg>.txt`, а доставляется первые 8 KiB и строка «Полный текст: <путь>». Агенты прочитают файл сами.

### 8.8 Журнал

`messages.jsonl` — сообщения с итоговым состоянием (`delivered` / `dropped` + причина). Он питает Activity, инспектор ребра и отладку. Последние 500 записей держатся в памяти Hub.

---

## 9. Управление канвасом агентами

### 9.1 Токены и скоупы

- **Токен ноды:** `kc1.<canvas>.<node>.<epoch>.<scope>.<mac>`, где `mac = HMAC-SHA256(secret, canvas|node|epoch|scope)`. Секрет лежит в `<config dir>/canvas.secret` (0600). Токены переживают рестарты и пересборки (терминалы тоже переживают). Отзыв — удаление ноды или `epoch++` («Rotate token» в меню ноды).
- **Скоупы** накопительные:

  | Скоуп | Что можно |
  |---|---|
  | `observe` | view, read, wait |
  | `talk` | + send в ноды, emit своего выхода |
  | `build` | + создавать ноды, удалять **свои**, соединять, двигать |
  | `admin` | + удалять любые ноды, менять настройки канваса |

  По умолчанию: Бригадир — `admin`; kou-агенты и харнесы, созданные человеком, — `build`; созданные агентом — `talk` (дать `build` может создатель с `build` и только в пределах `max_depth`); шелл — `talk`, а кто печатает в шелле, решает человек.
- Нода — «принципал». Каждое действие пишется с `actor` и видно в Activity («Triage spawned coder»).
- Честное замечание: токены — это **атрибуция и защита от ошибок агентов**, а не изоляция. Агент — процесс того же пользователя с Bash и может достучаться до локального API (§12).

### 9.2 Набор операций

Одна модель для CLI, MCP и инструментов kou (имена ниже — инструментов):

| Инструмент | Аргументы | Что делает |
|---|---|---|
| `CanvasView` | `{ detail?: "summary"\|"full" }` | Я (id, вид, прямоугольник, скоуп, brief); ноды (id, вид, пресет, название, статус, x, y, w, h, порты, worktree/ветка, программа, created_by); рёбра; подсказки свободного места справа/снизу от меня |
| `CanvasSpawn` | `{ kind \| preset, title, command?, cwd?, worktree?: {name?, base?} \| true, prompt?, config?, near?, side?: right\|below\|left\|above, x?, y?, w?, h?, connect?: { from?, to? } }` | Создаёт ноду, при необходимости сразу соединяет и шлёт первый промпт. Возвращает id и итоговый прямоугольник |
| `CanvasRemove` | `{ node, keep_worktree?: true, keep_session?: true }` | Удаление (свои — со скоупом `build`, любые — с `admin`) |
| `CanvasConnect` | `{ from: "node:port", to: "node:port", template?, mode?, deliver? }` | Ребро |
| `CanvasDisconnect` | `{ edge }` или `{ from, to }` | Удалить ребро |
| `CanvasMove` | `{ node, x?, y?, w?, h? }` | Раскладка |
| `CanvasSend` | `{ node, text?, submit?: true, keys?: ["C-c"], when?: "idle"\|"now" }` | Ввод как tmux send-keys |
| `CanvasRead` | `{ node, what?: "screen"\|"tail"\|"output"\|"answer", lines?: 80, wait?: "idle"\|"output"\|"exit", timeout_s?: 600 }` | Чтение, опционально после ожидания: «отправил задачу → жду → читаю» без поллинга |
| `CanvasEmit` | `{ port?: "out", text, data? }` | Явный выход своей ноды (режим `output: explicit`) |

Девять инструментов. Если замеры покажут, что модель путается, `Move` и `Disconnect` можно свернуть в `Connect` и `Spawn`.

### 9.3 CLI `kou-conveyor-canvas` (`kou-canvas`)

Новый небольшой Go-бинарь `cmd/kou-conveyor-canvas`. Собирается в `make build-web` рядом с раннером, добавляется в `programs` в `rebuild.go` и в `install.sh`. Сервер кладёт симлинк `kou-canvas` в `<cache>/kou-conveyor/canvas-bin/`. В окружении ноды:

- `KOU_CANVAS_BIN` — этот каталог; интеграция шелла добавляет его в начало `PATH` после файлов пользователя, без интеграции — env `PATH`;
- `KOU_CANVAS_CLI` — абсолютный путь, для хуков;
- `KOU_CANVAS_URL`, `KOU_CANVAS_TOKEN`, `KOU_CANVAS_ID`, `KOU_CANVAS_NODE`.

```
kou-canvas self                                   # кто я, скоуп, brief
kou-canvas view [--json]                          # граф с координатами
kou-canvas spawn codex --title worker-a --worktree fix-a --right-of self --prompt "…"
kou-canvas spawn command --title tests --cmd "go test ./..." --below self --connect-to self
kou-canvas spawn agent --title reviewer --worktree --prompt "Review what workers send you"
kou-canvas rm <node> [--keep-worktree]
kou-canvas connect <node>:out <node>:in [--template '…'] [--approve]
kou-canvas disconnect <edge>
kou-canvas move <node> --x 100 --y 200 [--w 760 --h 460]
kou-canvas send <node> "text" [--no-enter] [--now]
kou-canvas keys <node> C-c Escape Up Enter
kou-canvas read <node> [--screen|--tail N|--output|--answer] [--wait idle|output|exit] [--timeout 600]
kou-canvas emit [--port out] "text" [--data '{…}']
kou-canvas launch [--resume]                      # для пресетов (§6.2)
kou-canvas hook claude|codex                      # для хуков харнесов (§7.3, §7.4)
kou-canvas mcp                                    # stdio MCP-сервер (§9.5)
kou-canvas tool <name>                            # для инструментов плагина canvas-agent: JSON на stdin
```

Вывод по умолчанию человекочитаемый, `--json` — для скриптов. Без `KOU_CANVAS_TOKEN` команды, кроме `hook`, честно говорят «not on a canvas».

### 9.4 Инструменты kou: harness-плагин `canvas-agent`

- Это встроенный плагин **харнеса** `harness/plugin/builtin/canvas-agent/`, рядом с `guide`, а не веб-плагин: раннер не видит веб-встроенные плагины (§1). Состав:
  - `plugin.json` — `tools` из §9.2, каждый с `"run": ["kou-canvas", "tool", "<name>"]`; аргументы приходят JSON на stdin (существующий протокол инструментов);
  - `prompt` — короткий раздел системного промпта: что такое канвас и этикет (смотри `CanvasView` перед действиями, ставь ноды рядом с собой, один писатель на worktree, жди через `CanvasRead wait`, а не поллингом, пиши кратко — твой текст читают другие агенты, не удаляй чужие ноды);
  - `skills/kou-canvas/SKILL.md` — подробный справочник и рецепты: «воркер в worktree», «фан-аут на N воркеров и сбор», «ревью-цикл с approve».
- **Активация только на канвасе.** В манифест добавляется поле `"requires": { "env": ["KOU_CANVAS_TOKEN"] }` (§13). Плагин неактивен, если переменной нет: `Reason` = «only agents on a canvas have it». Обычные сессии, TUI и `-p` из терминала его не видят — ни лишних инструментов, ни токенов в промпте. `Discover` получает `Options.Getenv`; по умолчанию это `os.Getenv`.
- Схемы инструментов — единый источник в Go (`cmd/kou-conveyor-canvas/tools.go`). `plugin.json` генерируется `go generate`, тест сверяет, что они не разошлись. MCP-сервер берёт схемы из того же места.
- **Container-sandbox.** Изнутри контейнера `127.0.0.1` хоста недоступен. В v1 канвас-ноды kou с `sandbox=container` запускаются без `KOU_CANVAS_TOKEN`, то есть без инструментов канваса, и UI это показывает. Позже — unix-сокет API, примонтированный в контейнер.

### 9.5 MCP для Claude Code и Codex

- `kou-canvas mcp` — stdio MCP-сервер (JSON-RPC: `initialize`, `tools/list`, `tools/call`) с теми же инструментами §9.2, проксирующий в HTTP API с токеном из env.
- Реализация: официальный `github.com/modelcontextprotocol/go-sdk` (новая зависимость) или минимальная своя, ~300 строк. Решить в фазе 4, исходя из политики зависимостей.
- Подключается флагами запуска (§7.3, §7.4), без записи в `~/.claude` и `~/.codex`.

### 9.6 Размещение (координаты)

- `CanvasView` отдаёт прямоугольники всех нод и `free` — подсказки: «справа от тебя свободно с x=…, y=…», «снизу…».
- `place.go` вызывается, когда `x`/`y` не заданы:
  1. кандидаты — справа, снизу, слева, сверху от якоря (`near`, по умолчанию — нода-создатель) с зазором 40 px;
  2. при пересечении с другими нодами (с полем 32 px) кандидат сдвигается вдоль оси шагами 40 px;
  3. если не нашлось — спираль по сетке.

  Алгоритм детерминированный, протестирован.
- Если агент сам задал координаты и они пересекаются с существующими нодами — сервер сдвигает ноду и возвращает итоговые координаты. Флаг `exact: true` отключает сдвиг.
- Опционально (фаза 6) — `CanvasArrange { layout: "columns" | "tree" }`.

### 9.7 Бригадир (Foreman)

- Поле внизу страницы канваса — «Ask the foreman…». Первый промпт создаёт kou-ноду «Foreman» со скоупом `admin` и ролью «строит и обслуживает канвас по просьбе пользователя», ставит её у начала координат и отправляет туда промпт.
- Пример: «Сделай двух Codex-воркеров в worktree fix-a и fix-b и kou-ревьюера, соедини воркеров с ревьюером, раздай задачи из ISSUES.md». Бригадир строит граф, соединяет, раздаёт задачи и ждёт.
- Ответы Бригадира — в его ноде и короткой «облачной» подсказкой над полем.
- С пустого экрана Chat · Canvas черновик композера переносится сюда (§3.1).
- Названия-альтернативы: Dispatcher, Conductor. «Бригадир» лучше вписывается в метафору конвейера.

### 9.8 Ограждения автономии (настройки канваса)

| Настройка | Значения | По умолчанию |
|---|---|---|
| `autonomy.spawn` | `allow` (в пределах лимитов) \| `ask` (призрак ноды с Approve/Reject) \| `deny` | `allow` |
| `max_nodes` | максимум нод | 48 |
| `max_terminals` | максимум терминалов | 16 |
| `spawns_per_minute` | скорость спавна | 6 |
| `max_depth` | глубина спавна | 3 |
| удаление | агент удаляет только созданные им ноды (если не `admin`); worktree с изменениями — никогда | — |

Лента Activity с фильтром «только действия агентов» и кнопкой Undo у последних действий.

---

## 10. События и плагины

### 10.1 Секция `canvas` в `plugin.json`

```jsonc
{
  "name": "github-events",
  "version": "1.0.0",
  "description": "GitHub events for the canvas: issues opened or updated, through the gh CLI.",
  "canvas": {
    "sources": [
      {
        "id": "github-issues",
        "title": "GitHub issues",
        "description": "An event for every issue that matches a search, as it is opened or updated",
        "icon": "<svg …>",
        "run": ["./bin/github-issues"],
        "mode": "poll",                 // poll | stream
        "interval": "60s",              // для poll; минимум 10s
        "config": {                     // подмножество JSON Schema → форма инспектора
          "type": "object",
          "required": ["repo"],
          "properties": {
            "repo":   { "type": "string", "title": "Repository", "pattern": "^[^/\\s]+/[^/\\s]+$" },
            "search": { "type": "string", "title": "Search", "default": "is:open" },
            "events": { "type": "array", "title": "Events", "items": { "enum": ["opened", "updated", "closed"] }, "default": ["opened"] }
          }
        },
        "outputs": [ { "id": "opened", "title": "Issue opened" }, { "id": "updated", "title": "Issue updated" } ],
        "template": "Issue #{{data.number}}: {{data.title}}\n{{data.url}}\n\n{{data.body}}"
      }
    ],
    "harnesses": [ /* пресеты §7.2 */ ],
    "templates": "canvas-templates"     // каталог *.json — готовые канвасы
  }
}
```

- `harness/plugin` получает `Manifest.Canvas *Canvas` и валидацию:
  - `id` — как имена команд;
  - `run` — по правилам `tools` (`./` пути внутри плагина);
  - `interval` парсится, не меньше минимума;
  - `outputs` непустые;
  - схема — поддерживаемое подмножество: `string`, `number`, `integer`, `boolean`, `enum`, `array` of `string` / `enum`, `default`, `title`, `description`, `pattern`, `required`.
- Инспектор Plugins показывает «Canvas: 1 source, 2 harnesses» (поле в `pluginView`).
- Источники workspace-плагинов запускаются только в доверенном воркспейсе — существующее правило доверия.

### 10.2 Протокол источника

- **Запуск.**
  - Команда `run` выполняется в каталоге воркспейса.
  - Env: `KOU_CANVAS_URL`, `KOU_CANVAS_TOKEN` (скоуп только «emit своей ноды»), `KOU_CANVAS_ID`, `KOU_CANVAS_NODE`, `KOU_CANVAS_STATE_DIR` (постоянный каталог источника), `KOU_CONVEYOR_PLUGIN_DIR`, `KOU_CONVEYOR_WORKSPACE`.
  - stdin — одна строка JSON `{"config":{…},"node":"…","canvas":"…","first_run":true}`. В `stream` stdin остаётся открытым: EOF означает «остановись».
- **stdout — JSONL:**
  ```jsonc
  {"type":"event","port":"opened","key":"issue:42:opened","title":"#42 Crash on save","text":"…","data":{…}}
  {"type":"status","state":"ok","text":"watching owner/repo"}      // ok | error
  {"type":"log","text":"…"}
  ```
  stderr уходит в журнал ноды.
- **`poll`.** Сервер запускает команду раз в `interval`, она печатает новые события и выходит. **`stream`** — долгоживущий процесс.
- **Сервер:**
  - дедуп по `key`: LRU на 10 000 ключей, сохраняется в state dir;
  - лимит скорости;
  - backoff при падениях (1 с → 60 с) и статус `error` после 3 подряд;
  - остановка при удалении ноды, паузе канваса, выключении плагина (нода показывает «plugin off»).
- **Первый запуск** не заливает историю: по умолчанию курсор — «сейчас». Флаг `backfill` в конфиге — сознательная загрузка.
- Пересборка и рестарт сервера убивают источники; новые процессы продолжают с курсора, дедуп отбрасывает повторы.

### 10.3 Встроенные источники (плагин `canvas`)

| Источник | Что делает |
|---|---|
| `manual` | кнопка «Fire» с текстом — для отладки связок |
| `timer` | интервал или время суток; cron — позже |
| `files` | изменения по glob в воркспейсе; поллинг отпечатков, как `pluginWatch` (400 мс), с дебаунсом |
| `webhook` | локальный URL `POST /api/canvas/hooks/<id>` (секрет в пути). Для GitHub из интернета нужен туннель (`gh webhook forward`, cloudflared) — в UI так и написано |

### 10.4 Пример: GitHub issues

`examples/plugins/github-events/` — пример плагина, как `scratchpad`: `plugin.json` выше и `bin/github-issues` (poll):

1. прочитать конфиг из stdin;
2. взять курсор из `$KOU_CANVAS_STATE_DIR/cursor`;
3. выполнить `gh issue list --repo <repo> --search "<search> updated:>=<cursor>" --state all --json number,title,url,body,labels,author,createdAt,updatedAt,state --limit 50`;
4. для каждого issue напечатать `{"type":"event","port": opened|updated, "key": "<number>:<updatedAt>", …}`;
5. сохранить новый курсор.

Требуется `gh auth status` — иначе `{"type":"status","state":"error","text":"gh is not signed in"}`. Скрипт делаем переносимым (Python 3 или Go), а не на BSD/GNU `date`.

Типичная связка: GitHub issues → Triage (kou) → Triage сам спавнит Codex в worktree `fix-<n>` с текстом issue → выход Codex → Reporter (kou), который комментирует issue через `gh` (§17.C, сценарий 2).

### 10.5 Веб-расширения канваса (точки вклада)

| Точка | Элемент | Для чего |
|---|---|---|
| `canvas.node` | `{ kind \| preset, order, create(node, ctx) → { node, update(node), shown(), hidden(), lod(level), dispose() } }` | свой рендер тела ноды: например, карточки issue в ноде GitHub |
| `canvas.add` | `{ id, group, title, icon, order, shown(canvas), create(at) }` | пункты меню «+ Add» |
| `canvas.template` | `{ id, title, description, build() → ops }` | шаблоны на пустом канвасе |
| `canvas.inspector` | `{ kinds, order, render(node, ctx) }` | секции инспектора ноды |

Сервис `canvas`: `open(id)`, `create({ template, title })`, `current()`, `addNode(spec, at)`, `select(ids)`, `focusNode(id)`, `fit()`.

### 10.6 Шаблоны

Канвас без `runtime` — переносимый JSON. `⋯ → Save as template` пишет в `.harness/canvas-templates/<name>.json`; плагины поставляют свои через `canvas.templates`. Пустой канвас показывает галерею:

- **Pair** — кодер Claude в worktree + kou-ревьюер, ребро через approve;
- **Fan-out** — Бригадир + N воркеров + сборщик;
- **Issue triage** — по примеру выше;
- **Empty**.

---

## 11. Веб-плагин `canvas`

### 11.1 Файлы

```
cmd/kou-conveyor-web/plugins/canvas/
  plugin.json        web: script/style, after: [layout, session, ui, terminal, markdown, files, models, session-list];
                     canvas.harnesses (встроенные пресеты), canvas.sources (manual/timer/files/webhook)
  web/canvas.js      вход: вид и роут, переключатель Chat·Canvas, строки в списке сессий, палитра,
                     /canvas, сервис canvas, точки вклада
  web/model.js       клиентская модель: снимок + SSE, применение ops, оптимистичные правки, undo-стек
  web/surface.js     бесконечная поверхность: pan/zoom, выделение (клик, Shift, рамка), drag, resize, сетка
  web/edges.js       SVG-слой рёбер, протягивание от порта, анимация сообщений
  web/lod.js         уровни детализации и бюджет «живых» терминалов
  web/nodes/terminal.js, nodes/agent.js, nodes/source.js, nodes/note.js
  web/forms.js       JSON-схема → форма (поддерживаемое подмножество §10.1)
  web/inspector.js   правый выезжающий инспектор
  web/rail.js        рейл: канвасы, Outline, Activity
  web/foreman.js     поле Бригадира
  web/canvas.css
```

### 11.2 Техника поверхности

- **Мир** — один `div` с `transform: translate(x, y) scale(z)` (`will-change: transform`). Ноды — абсолютно позиционированные DOM-элементы. Рёбра — один SVG в том же мире. Всё строится через `h()` / `svg()` ядра: ни одного `innerHTML` (правило кокпита — §1, `dom.js`). CSSOM (`el.style.transform`) разрешён текущей CSP — терминал уже так делает.
- **Перерисовка** батчится в `requestAnimationFrame`. Во время drag двигается только перетаскиваемая нода и её рёбра. Для других вкладок позиция шлётся операцией `node.update` до 10 раз в секунду, финальная — по отпусканию.
- **Масштаб** 0.1–2.0. Сетка — фоновый паттерн. LOD (§6.6): при масштабе < 0.35 у нод видна только шапка.
- **Состояние** в `cockpit.hot.data`: при перезагрузке плагина канвас не моргает, как у scratchpad. Вьюпорт канваса — в `prefs`.
- **Почему своя поверхность, а не библиотека.**
  - React Flow / Svelte Flow / tldraw тянут фреймворк, а в проекте ванильный JS без сборки.
  - litegraph рисует ноды на `<canvas>`, и DOM-терминалы туда не встроить.
  - Drawflow строит ноды через HTML-строки (`innerHTML`) — против правил безопасности кокпита.

  Нужное ядро — pan/zoom, drag, порты, Безье — порядка 1.5–2 тыс. строк в стиле остального кода.

### 11.3 Вид, роут, переключатель — без правок чужих плагинов

```js
cockpit.contribute('layout.view', { id: 'canvas', title: 'Canvas', order: 15, rail, page, select: openLast, shown, hidden });
cockpit.routes.register({ id: 'canvas', priority: 10,
  match: (hash) => /^#\/w\/([A-Za-z0-9-]{1,80})\/c\/([A-Za-z0-9-]{1,128})$/.exec(hash),
  enter: ([, ws, id]) => { selectWorkspace(ws); session.ensureView(); layout.show('canvas'); model.open(ws, id); } });
// Chat · Canvas: смонтирован в bar.crumbs, виден только у свежего вида Sessions
cockpit.ui.mount('bar.crumbs', { id: 'session-mode', order: 5, node: modeSwitch });
cockpit.on('render', () => { modeSwitch.hidden = !(store.get('view') === 'sessions' && session.view()?.fresh); });
```

На странице канваса (`data-page`) боковые панели layout скрыты — это существующее поведение. Свойства нод показывает собственный инспектор канваса. Показ сайдбара рядом с канвасом — возможное улучшение layout позже, не для v1.

---

## 12. Безопасность и ограждения

- **Модель доверия не меняется.**
  - Сервер локальный: allowlist Host, `CrossOriginProtection` на не-GET, CSP.
  - Маршруты UI канваса защищены так же, как все остальные.
  - Агентские `/api/canvas/*` требуют токен.
  - Терминалы и раны и сейчас может запустить любой локальный процесс того же пользователя — канвас эту поверхность не расширяет, но делает действия агентов атрибутируемыми и ограниченными.
- **Инъекции во вставку.** Очистка §6.3: ESC, C1 и C0 удаляются из текстовых доставок. Управляющие последовательности можно послать только явным `keys`.
- **Токен в env** видят все программы в терминале ноды — та же граница доверия, что у пользовательского шелла. Скоуп минимален, токен привязан к ноде, отзывается через `epoch`. В argv (видно в `ps`) токены не кладём.
- **Бинд не на loopback** (`-address 0.0.0.0:…`, `anyHost`). Канвас-API с токенами становится доступен по сети, как и всё остальное. UI предупреждает при включении канваса на таком сервере.
- **Доверие к воркспейсу.** Источники и пресеты из `.harness/plugins` работают только в доверенном воркспейсе.
- **Автономия.** Лимиты и режимы §9.8, защита от циклов §8.5, пауза §8.6. Worktree с изменениями никогда не удаляется автоматически.
- **Деньги и лимиты подписки.** Раны канваса идут через то же соединение и аккаунты. Позже: бюджет канваса в окнах подписки — см. `audit-234b16c9/DESIGN-long-running.md` §3.1.

---

## 13. Изменения в существующем коде (и почему пайплайн не ломается)

| Файл / пакет | Изменение | Почему безопасно |
|---|---|---|
| `cmd/internal/terminal/terminal.go`, `scan.go`, `shell.go`, `handover_*.go` | `Spec.Owner/Env`, `Info.Owner`, `List` без владельцев, `ListOwned`, `Paste`, `Keys`, `Observe`, `Tail`; OSC 133 и DECCKM в сканере; `Owner`/`Env` в записи handover | Нулевые значения = текущее поведение; сайдбар видит тот же список; новые методы не вызываются без канваса; тесты пакета расширяются, а не меняются |
| `cmd/kou-conveyor-web/plugins/terminal/shell/bash/kou.bash`, `fish/kou.fish` | OSC 133 C/D; `KOU_CANVAS_BIN` в PATH, если задан | Метки 133 — стандарт (Ghostty, iTerm2, VS Code), restty их понимает; без `KOU_CANVAS_BIN` PATH не трогается |
| `cmd/kou-conveyor-web/plugins/terminal/web/{terminal,view}.js` | сервис `terminal.mount(…)` (вынос транспорта и инициализации одной панели) | Рефакторинг внутри плагина; вкладки сайдбара — на том же коде |
| `cmd/kou-conveyor-web/server.go` | поле `canvas`, маршруты, `startCanvas()`, flush в `close` | Новые маршруты; при `-canvas=off` — no-op движок |
| `cmd/kou-conveyor-web/runs.go` | `RunEnv`, `RunStarted`, `RunFinished` (§5.5) | Для сессий без канваса — пустые вызовы |
| `cmd/kou-conveyor-web/queue.go` | вынос `enqueue` из `handleEnqueue` | Поведение то же, покрыто `queue_test.go` |
| `cmd/kou-conveyor-web/rebuild.go` | `s.canvas.Flush()` перед `exec`; `kou-conveyor-canvas` в `programs` | Flush пустого движка — no-op |
| `cmd/kou-conveyor-web/main.go` | флаг `-canvas` (`on`/`off`, env `KOU_CONVEYOR_CANVAS`); в `/api/config` — `canvas: bool` | По умолчанию — см. фазы: `off` до фазы 3, затем `on` |
| `cmd/internal/cockpit/runner.go` | `Request.Env []string` — дописывается к env рана | Внутреннее поле, в JSON раннера не уходит |
| `cmd/internal/cockpit/manage.go`, `sessions.go` | `Meta.Canvas`, `Meta.Node`; `SessionInfo.Canvas`; в ответе `/sessions/{id}` — `canvas`, `node` | Поля с `omitzero`; старые `.meta` читаются как прежде; TUI их игнорирует |
| `cmd/internal/agentrunner/sandbox.go` | `ensureWorktree` → `cmd/internal/worktree` | Чистый перенос, тесты раннера как есть |
| `harness/plugin/plugin.go`, `discover.go` | `Manifest.Canvas`, `Manifest.Requires{Env}`, валидация; `Options.Getenv` | Новые необязательные поля. Но парсинг строгий (`RejectUnknownMembers`): плагин с секцией `canvas` старые бинарники отвергнут как «unknown member». Это касается только новых плагинов, а cockpit и раннер собираются вместе |
| `harness/plugin/builtin/canvas-agent/` | новый встроенный плагин (инструменты, prompt, skill) | Неактивен без `KOU_CANVAS_TOKEN` (`requires.env`) — обычные раны его не видят |
| `cmd/kou-conveyor-web/plugins/session-list/web/session-list.js` | точка `session-list.rows` и скрытие сессий с `canvas` | Без вкладчиков — тот же список |
| `cmd/kou-conveyor-web/plugins/header/web/header.js` | крошка «◧ канвас ▸ нода» для сессий с `canvas` (или монтирует сам плагин `canvas` в `bar.crumbs`) | Предпочтительно второй вариант — ноль правок header |
| `Makefile`, `install.sh` | сборка и установка `kou-conveyor-canvas` | — |
| `docs/plugins.md`, `README.md` | разделы Canvas, секция `canvas` манифеста, точки, сервис | — |

**Флаги и выключатели:**

1. Плагин `canvas` выключается в Plugins: UI исчезает целиком — переключатель, вид, роуты, строки в списке.
2. Флаг сервера `-canvas=off`: движок no-op, маршруты отвечают 404, `/api/config` говорит `canvas:false`, и плагин ничего не показывает.

Регрессионный контроль: `make test`, `make check`, существующие тесты терминалов, очереди и ранов без правок, плюс «дымовой» прогон кокпита с выключенным плагином (§15).

---

## 14. Фазы работ

Оценки — грубые, в днях одного разработчика, для порядка величин.

### Фаза 0 — спайки (3–4 дня) → `plans/canvas-spikes.md` с решениями

| # | Вопрос | Как проверяем |
|---|---|---|
| 0.1 | restty на канвасе | 8/16/24 экземпляров — FPS, память, потеря WebGL/WebGPU-контекстов; поведение в `transform: scale()`; `autoResize: false` и ручной размер; мышь и выделение при масштабе ≠ 1. Итог — правила LOD и режима фокуса |
| 0.2 | Доставка текста | zsh, bash, fish, Claude Code, Codex, OpenCode, aider: bracketed paste + Enter с задержками 0/30/80/150 мс; многострочный и длинный (10 KiB) текст, unicode. Итог — параметры `input` пресетов |
| 0.3 | Хуки и MCP | `claude --settings` (поля `SessionStart`/`UserPromptSubmit`/`Stop`/`Notification`, `transcript_path`), `--mcp-config` stdio, `--session-id`/`--resume`; Codex `-c notify=[…]` (`agent-turn-complete`, `last-assistant-message`, `thread-id`), хуки Codex, `-c mcp_servers.*` и проброс env, `codex queue` с идущей TUI |
| 0.4 | Серверный экран | `charmbracelet/x/vt` против `hinshun/vt10x` против libghostty-vt WASM через wazero (тот же движок, что у restty): точность на TUI Claude/Codex, CPU на replay 2 MiB. Нужен для `read --screen` и снимков LOD |
| 0.5 | OSC 133 C/D | для bash (`PS0` + `PROMPT_COMMAND`) и fish (`fish_preexec`/`fish_postexec`); захват вывода между C и D в сканере |

### Фаза 1 — каркас (6–8 дней)

**Сервер:**

- `cmd/internal/canvas`: doc, store, ops, hub, SSE, place;
- `canvas.go` — маршруты UI;
- `terminal.Spec.Owner/Env`, `List` без владельцев, handover с `Owner`;
- виды `terminal` (пресеты `shell`, `command`) и `note`;
- запуск и привязка PTY при `node.add`, `CloseAfter` и Undo при `node.remove`;
- persist, перезагрузка live-канвасов;
- флаг `-canvas` (по умолчанию `off`).

**Веб:**

- плагин `canvas`: вид, страница, роут, поверхность (pan, zoom, выделение, drag, resize, сетка);
- ноды terminal (через сервис `terminal`, сделать рефакторинг) и note;
- шапка (название, масштаб, Add), рейл (канвасы, Outline);
- **Chat · Canvas** на свежей сессии;
- строки канвасов в Sessions (точка `session-list.rows`), палитра, `/canvas`.

**Готово, когда:**

- из New Session → Canvas создаю канвас, добавляю 3 терминала и работаю в них;
- перезагрузка страницы → та же раскладка и живые шеллы;
- пересборка сервера → шеллы на месте;
- удаление ноды → шелл гаснет через 15 с, Undo возвращает;
- плагин выключен → ни следа в UI;
- `make test` и `make check` зелёные.

### Фаза 2 — рёбра и роутер (5–7 дней)

- Порты и рёбра в UI: протягивание, инспектор, шаблоны с предпросмотром, режимы, approve.
- `router.go`: fan-out/in, pending на диске, заголовок происхождения, большие сообщения, защита от циклов, лимиты, предохранитель, журнал.
- `Paste` и `Keys` с очисткой; OSC 133 → статус и выход shell-нод; `exit`-порт; idle-эвристика.
- SSE `status`/`message`/`notice`, анимация рёбер, бейджи pending, Activity.
- Источник `manual` для отладки.
- Live/Paused.

**Готово, когда:**

- `echo hi` в шелле A → вывод приходит в шелл B через шаблон `echo "got: {{text}}"`;
- цикл A↔B останавливается на `max_hops` с notice;
- approve-ребро держит сообщение до подтверждения;
- после рестарта сервера pending доставляется.

### Фаза 3 — харнесы (7–10 дней)

- **kou-нода** (§7.1):
  - сессия ноды, `Meta.Canvas/Node`;
  - вынос `enqueue`, `RunEnv`/`RunStarted`/`RunFinished`;
  - `/transcript`, события `agent` в SSE;
  - лента и инпут в вебе, Open session и обратная крошка;
  - `sandbox: worktree`.
- Пресеты как данные (`canvas.harnesses` встроенного плагина): **Claude Code**, **Codex**, generic (OpenCode, aider).
- Режим запуска `launcher`, CLI в минимальном объёме (`launch`, `hook claude|codex`, `emit`), каталог `canvas-bin`, `KOU_CANVAS_*` в env терминалов.
- `cmd/internal/worktree`; терминальные ноды в worktree; Remove worktree с проверкой.
- Статус `waiting`; Resume харнеса после краха.
- Флаг `-canvas` по умолчанию → `on`.

**Готово, когда:**

- ход Claude Code завершился → последний ответ ушёл kou-ревьюеру, тот отработал, ответ вернулся Claude через approve;
- нода Claude после `kill` сервера без handover продолжает сессию через Resume;
- kou-нода в worktree коммитит в свою ветку.

### Фаза 4 — API агентов и Бригадир (6–8 дней)

- Токены и скоупы, маршруты `/api/canvas/*`, `place.go` в API, `wait` (long-poll).
- CLI целиком: `view`, `spawn`, `rm`, `connect`, `disconnect`, `move`, `send`, `keys`, `read`, `self`, `tool`.
- MCP-сервер `kou-canvas mcp`; подключение к пресетам Claude и Codex.
- Harness-плагин `canvas-agent`: инструменты, prompt, skill, `requires.env` в `harness/plugin`, генерация схем.
- Автономия (§9.8): лимиты, `ask`-режим с призраками нод, Activity с actor и Undo.
- Поле Бригадира.

**Готово, когда:**

- Бригадир по одному промпту строит «2 Codex-воркера в worktree + kou-ревьюер + связи» с разумной раскладкой без пересечений;
- Claude Code через MCP спавнит рядом с собой терминал `go test ./...` и читает его вывод через `read --wait exit`;
- агент со скоупом `talk` получает отказ на `spawn`; удаление чужой ноды без `admin` отклоняется.

### Фаза 5 — события и плагины (5–7 дней)

- `Manifest.Canvas`: валидация, отображение в инспекторе Plugins.
- Супервизор источников (poll/stream, backoff, дедуп, state dir, остановка при выключении плагина или паузе).
- Встроенные `timer`, `files`, `webhook`.
- `examples/plugins/github-events`.
- Точки `canvas.node`, `canvas.add`, `canvas.template`, `canvas.inspector`; шаблоны и галерея.
- `docs/plugins.md` → раздел Canvas.

**Готово, когда:**

- установка примера → «GitHub issues» появляется в Add с формой из схемы;
- новый issue → событие → ран триажного агента;
- выключение плагина → источник остановлен, нода «plugin off»;
- повторный запуск сервера не дублирует события.

### Фаза 6 — полировка (по мере надобности)

- Снимки экрана с серверного эмулятора для LOD, мини-карта, поиск нод, мультивыделение и выравнивание, фреймы-группы, `CanvasArrange`.
- Фильтры на рёбрах; экспорт и импорт.
- Outline-режим для телефона; клавиатурная навигация и a11y.
- Производительность на 200 нод; бюджет канваса в окнах подписки.
- Unix-сокет API для container-sandbox; `append_system_prompt` в раннере.

**Определение «готово» для всей фичи:**

- сценарии §17.C проходят руками;
- `make test` и `make check` зелёные;
- выключенный плагин / `-canvas=off` дают ровно текущий кокпит;
- документация обновлена.

---

## 15. Тестирование

| Уровень | Что | Где |
|---|---|---|
| Unit (Go) | валидация документа, ops (атомарность, LWW, 409), place (детерминизм, без пересечений), шаблоны, роутер (fan-out/in, hops, лимиты, предохранитель, большие сообщения), токены (HMAC, epoch, скоупы) | `cmd/internal/canvas/*_test.go` |
| Unit (Go) | `Paste` (bracketed / нет, `\n→\r`, задержка Enter, **очистка `ESC[201~`**), `Keys` (DECCKM), сканер OSC 133 C/D и захват вывода, `Observe` не блокирует `pump`, `List` без владельцев, handover с `Owner` | `cmd/internal/terminal/terminal_test.go` (рядом с `TestScannerFollowsTitleDirectoryAndModes`, `TestAdoptTakesUpAShellHandedOver`) |
| Unit (Go) | сборка argv и файлов пресетов (golden), brief, разбор входов хуков Claude/Codex (фикстуры JSON) | `cmd/internal/canvas`, `cmd/kou-conveyor-canvas` |
| Unit (Go) | `Manifest.Canvas`/`Requires`: валидация, активация по env | `harness/plugin/plugin_test.go` |
| Интеграция | CRUD и SSE канваса; терминальная нода с `/bin/sh` (`quietShell`) — запуск, WS-подключение, ввод, OSC 133; маршрут shell→shell; kou-нода с поддельным раннером (`cockpittest.Runner`) → `RunFinished` → доставка; рестарт сервера → перезагрузка live-канваса и pending; токены (401/403); лимиты | `cmd/kou-conveyor-web/canvas_test.go` на `newHarness` |
| Интеграция | источник-скрипт: stream и poll, падение и backoff, дедуп после рестарта | `cmd/internal/canvas/source_test.go` с временным скриптом |
| CLI / MCP | команды против `httptest`-сервера; JSON-RPC: `initialize`, `tools/list`, `tools/call`, ошибки | `cmd/kou-conveyor-canvas/*_test.go` |
| Регрессия | `make test`, `make check`; тесты терминалов, очереди, ранов без правок; кокпит с выключенным `canvas` | CI / руками |
| Веб (руками) | чек-лист на каждую фазу: Chat · Canvas, pan/zoom, LOD, фокус терминала, рёбра, approve, Undo, перезагрузка, пересборка, узкий экран | `plans/canvas-qa.md` (заводится в фазе 1) |
| Веб (опционально) | Playwright-сценарий «создать канвас → терминал → ребро → доставка», вне `make test` (в репозитории нет JS-инфраструктуры) | `cmd/kou-conveyor-web/e2e/` |

---

## 16. Риски и открытые вопросы

| # | Риск / вопрос | Что делаем |
|---|---|---|
| 1 | Много restty = много GPU-контекстов; браузер теряет старые | LOD, бюджет живых терминалов (8, LRU), снимки; спайк 0.1 |
| 2 | Масштаб ломает измерения restty, мышь и выделение | `autoResize: false`, фиксированная сетка, мышь только при 100 %, режим фокуса; запасной вариант — оверлей в экранных координатах |
| 3 | Определить «ход закончился» у TUI без хуков | Хуки и notify для Claude и Codex; для прочих — простой и `ready`-regex; по умолчанию generic = «приёмник» |
| 4 | Флаги внешних CLI меняются | Пресеты — данные в плагине; `check` и `min_version`; «Doctor» в меню ноды; спайк 0.3 перед фазой 3 |
| 5 | Переписка агентов ушла в цикл и жжёт лимиты | `max_hops`, approve на циклах по умолчанию, лимиты скорости, предохранитель, Paused, бейдж live; позже бюджет в окнах подписки |
| 6 | Человек и агент печатают одновременно | Атомарные вставки, плашка «typing», режимы `shared`/`human`/`agent`, доставка по `idle` |
| 7 | Агентский API — не изоляция | Явно задокументировано; скоупы для атрибуции и защиты от ошибок; предупреждение при бинде не на loopback |
| 8 | Container-sandbox не достаёт до API | v1: инструменты канваса выключены, UI это говорит; v2: unix-сокет в контейнер |
| 9 | Пересборка или рестарт посреди потока | pending и журнал на диске, курсоры и дедуп источников, Resume харнесов |
| 10 | Строгий парсинг `plugin.json` в старых бинарниках | Плагины с `canvas` требуют свежих бинарников — указать в docs; cockpit и раннер и так собираются вместе |
| 11 | Скрывать ли сессии агентов из Sessions | По умолчанию скрыты, есть переключатель; решить по отзывам |
| 12 | Названия: Chat · Canvas, Foreman | Финализировать до фазы 1 (§3.1, §9.7) |
| 13 | Канвас и боковые панели (Files, Changes) на одной странице | v1 — нет (страница вида прячет панели); позже — опция layout «page with panels» |
| 14 | Рост файлов (журналы, большие сообщения) | Ротация журнала, чистка `messages/` старше N дней при удалении канваса |
| 15 | Канвас в TUI | Не в v1. Дочерние сессии и так видны в пикере TUI как обычные |

---

## 17. Приложения

### A. Встроенный `plugin.json` плагина `canvas` (фрагмент)

```jsonc
{
  "name": "canvas",
  "description": "The canvas: terminals, agents and events on a board without edges, wired output to input. New session → Canvas.",
  "web": { "script": "web/canvas.js", "style": "web/canvas.css",
           "after": ["layout", "session", "ui", "terminal", "markdown", "files", "models", "session-list"] },
  "canvas": {
    "harnesses": [
      { "id": "shell", "title": "Shell", "launch": "shell", "status": ["osc133"], "output": "osc133" },
      { "id": "command", "title": "Command…", "launch": "type", "status": ["osc133"], "output": "osc133",
        "config": { "type": "object", "required": ["command"], "properties": { "command": { "type": "string", "title": "Command" } } } },
      { "id": "claude-code", "title": "Claude Code", "icon": "✻",
        "command": ["claude"], "launch": "launcher",
        "session_arg": ["--session-id", "{{runtime.agent_session}}"],
        "args": ["--settings", "{{files.settings}}", "--mcp-config", "{{files.mcp}}",
                 "--append-system-prompt", "{{brief}}", "--permission-mode", "{{config.permission_mode}}"],
        "files": {
          "settings": { "hooks": {
            "SessionStart":     [{ "hooks": [{ "type": "command", "command": "\"${KOU_CANVAS_CLI:-kou-canvas}\" hook claude", "timeout": 5 }] }],
            "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "\"${KOU_CANVAS_CLI:-kou-canvas}\" hook claude", "timeout": 5 }] }],
            "Stop":             [{ "hooks": [{ "type": "command", "command": "\"${KOU_CANVAS_CLI:-kou-canvas}\" hook claude", "timeout": 5 }] }],
            "Notification":     [{ "hooks": [{ "type": "command", "command": "\"${KOU_CANVAS_CLI:-kou-canvas}\" hook claude", "timeout": 5 }] }] } },
          "mcp": { "mcpServers": { "kou-canvas": { "command": "kou-canvas", "args": ["mcp"] } } }
        },
        "input": { "paste": "bracketed", "newline": "cr", "submit": "\r", "submit_delay_ms": 80 },
        "status": ["hooks"], "output": "hooks",
        "resume": ["--resume", "{{runtime.agent_session}}"],
        "config": { "type": "object", "properties": {
          "permission_mode": { "type": "string", "title": "Permissions", "enum": ["default", "acceptEdits", "plan", "bypassPermissions"], "default": "default" } } },
        "check": ["claude", "--version"] },
      { "id": "codex", "title": "Codex", "icon": "◎",
        "command": ["codex"], "launch": "launcher",
        "args": ["-c", "notify=[\"kou-canvas\",\"hook\",\"codex\"]",
                 "-c", "mcp_servers.kou-canvas.command=\"kou-canvas\"", "-c", "mcp_servers.kou-canvas.args=[\"mcp\"]"],
        "input": { "paste": "bracketed", "newline": "cr", "submit": "\r", "submit_delay_ms": 80 },
        "status": ["notify", "idle"], "idle_ms": 8000, "output": "notify",
        "resume": ["resume", "{{runtime.agent_session}}"],
        "check": ["codex", "--version"] },
      { "id": "opencode", "title": "OpenCode", "command": ["opencode"], "launch": "launcher",
        "input": { "paste": "bracketed", "submit": "\r", "submit_delay_ms": 80 }, "status": ["idle"], "idle_ms": 4000, "output": "none" }
    ],
    "sources": [
      { "id": "manual", "title": "Manual", "builtin": true, "outputs": [{ "id": "out", "title": "Fired" }] },
      { "id": "timer", "title": "Timer", "builtin": true, "outputs": [{ "id": "out", "title": "Tick" }],
        "config": { "type": "object", "properties": { "every": { "type": "string", "default": "15m" }, "text": { "type": "string" } } } }
    ]
  }
}
```

(`builtin: true` — источник реализован в Go внутри движка, без процесса. Значения флагов Claude и Codex — **проверить в спайке 0.3**.)

### B. Как выглядит работа агента из терминала

```sh
$ kou-canvas view
canvas «Issue triage» (live) · you: n_p0r4cz «coder» ✻ claude-code · access talk
  n_k3d9qa  ⚡ GitHub issues   source   idle   (0,0 320×220)       out:opened → Triage
  n_7hx2pm  ◆ Triage           agent    busy   (420,0 440×560)     out → coder
  n_p0r4cz  ✻ coder (you)      terminal busy   (940,120 760×520)   ⎇ kou/fix-42
free: right of you at (1740,120) · below you at (940,680)

$ kou-canvas spawn command --title tests --cmd "go test ./..." --below self --connect-to self
created n_t5w1rb «tests» at (940,680 760×460) · edge e_m4k: tests:out → coder:in

$ kou-canvas read n_t5w1rb --output --wait exit --timeout 900
exit 1 · 214 lines
--- FAIL: TestSaveCrash (0.02s) …
```

### C. Сценарии (приёмка)

1. **Пара «кодер + ревьюер».**
   - Шаблон Pair: Claude Code в worktree `pair`, kou «Reviewer».
   - Ребро coder:out → reviewer:in в режиме `auto`, обратное — `approve`.
   - Пользователь даёт задачу кодеру. Его ход уходит ревьюеру, ответ ревьюера ждёт подтверждения и уходит кодеру. Цикл идёт до «LGTM».
2. **Триаж issues.**
   - GitHub issues (`label:bug`) → Triage (kou, `build`).
   - На каждое issue Triage решает: дубликат — пишет вывод; реальный баг — `CanvasSpawn codex --worktree fix-<n> --prompt "<issue>" --connect to Reporter`.
   - Reporter (kou) по выходу Codex комментирует issue через `gh issue comment`.
   - Канвас Live, вкладка закрыта — работа идёт. Утром Activity показывает, что было.
3. **Бригадир и фан-аут.**
   - Пользователь: «Мигрируй пакеты a, b, c на новый API параллельно, потом общий ревью».
   - Бригадир спавнит 3 Codex в worktree `mig-a/b/c` с задачами, kou-сборщика, соединяет воркеров со сборщиком.
   - Через `CanvasRead --wait idle` дожидается всех и пишет итог.
   - Пользователь видит всё на канвасе и может зайти в любой терминал руками.
4. **REPL агент ↔ шелл.**
   - kou-агент с инструментом `CanvasSend`/`CanvasRead` гоняет долгую команду в терминальной ноде (сервер разработки) и читает её вывод.
   - Человек параллельно смотрит тот же PTY и может нажать `C-c`.
