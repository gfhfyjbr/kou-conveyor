# Долгие запуски в kou-conveyor — v3

Статус: **RFC, не описание уже реализованного режима**. Редакция от 2026-10-02.
Переработка v2 после архитектурного ревью; исходный разбор v1 — в
[`DESIGN-long-running-v1.md`](DESIGN-long-running-v1.md), данные сессии `234b16c9` — в
[`AUDIT.md`](AUDIT.md). Новые имена команд, событий и полей ниже — проектируемые контракты,
а не обещание, что соответствующий API уже существует.

Сохраняются два требования пользователя: бюджет — в **окнах лимитов подписки**, не в долларах;
**уровень B**, изменение кода харнеса, — штатная эскалация из рабочей сессии, но только с явного
разрешения пользователя. Сохраняются основные решения v2: один Writer, файловая память,
harness-recorded verify, чистый Reviewer, rollover и цикл design → run → reflect.

Главное изменение v3:

> **Модель предлагает намерения и объяснения. Харнес единолично фиксирует факты исполнения,
> принимает переходы состояния и применяет подтверждённый пользователем контракт.**

Это не запрет модели действовать свободно внутри задачи. Это запрет одному и тому же агенту
одновременно менять результат, критерий его приёмки и историю доказательств.

---

## 0. Что уточнено относительно v2

| Решение v2 | Проблема | Решение v3 |
|---|---|---|
| Модель редактирует `state.json`, харнес пишет в него `verify_runs` | Несколько писателей; схема JSON не защищает владельца поля; полный Write может подменить историю | Один reducer в харнесе; модель отправляет типизированные изменения с revision; `state.json` — восстанавливаемая проекция |
| Pass позже старта задачи разрешает `done` | После pass код мог измениться; verify-команда могла быть ослаблена | Pass привязан к immutable snapshot, версии контракта и approved verify-манифесту; приёмка — отдельный переход |
| `output_sha` закрывает objective hacking | Хеш доказывает целостность вывода, не качество проверки или реализации | Доверенный запуск + неизменяемые acceptance gates + Reviewer; отдельно обозначены пределы доказательства |
| Journal — append-only, но доступен через Write | Конвенция, а не механизм; авто-строки и модель могут терять записи друг друга | Единственный append через харнес; Markdown генерируется из событий |
| Judge после каждого хода останавливает или продолжает run | Не определён ход; цена полного транскрипта не измерена; ошибочная оценка влияет на управление | Hard guards детерминированы; LLM Judge консультативный, с bounded input, timeout и measured overhead |
| «Нет Edit/Write» означает отсутствие прогресса | Ошибочная остановка research; бессмысленная правка, наоборот, обходит правило | Progress зависит от kind задачи и новых evidence; есть временной watchdog независимо от компакций |
| Rollover переназначает running-операции | ID операции не означает безопасного переноса процесса; возможны дубликаты | MVP делает rollover только после quiescence; перенос работающих операций — отдельное позднее расширение |
| Sonnet/Haiku гарантированно не тратят Opus-окно | Scoped-окно может сосуществовать с общими лимитами; правила провайдера меняются | Каталог описывает все известные binding windows и уверенность mapping; неизвестность не считается бесплатностью |
| Резерв и доля weekly без определения единиц | Непонятно: абсолютная utilization, дельта или доля остатка? | Явные `reserve_percent`, `max_additional_pp`, reset epoch и freshness (§3.1) |
| Fallback на другую модель при упоре в окно | Противоречит pinning модели и может нарушить thinking/cache-контракт | Другая модель запускается новой ролью/сессией с handoff, никогда не подменяется молча |
| Плагин активируется автоматически в trusted workspace | Workspace trust не является одобрением каждого нового executable tool | Для long-running run — approved bundle по digest и capabilities; черновик плагина сам не активируется |
| Одна измеримая метрика | Coverage можно улучшать ценой качества или массовых skipped | Primary metric + quality gates + invariants + stop conditions |
| Replay/benchmark уровня B — последний этап | Эскалацию нечем объективно проверить до этого этапа | Детерминированные replay и fault injection начинаются до оркестрации ролей |

### 0.1. На что опираемся в текущем коде

Срез локального рабочего дерева на 2026-10-02: `c788518` + незакоммиченные изменения. Это не утверждение
о состоянии опубликованного релиза; перед реализацией необходимо заново проверить соответствующие места.

| Уже есть | Чего это пока не гарантирует |
|---|---|
| Append-only `sessionstore`, `Resume`, `ItemFork`, история операций | Нет описанного здесь goal/chain-контракта, единой транзакции rollover и межпроцессного Writer lease. `Store` явно не сериализует методы для одного session ID |
| `coordinator.Verifier`, операция с `VerificationMarker`; `cmd/internal/agentrunner/verify.go` читает `.harness/verify.json`, plugin verify и `KOU_CONVEYOR_VERIFY` | Проверки работают на завершение промпта, не на immutable task candidate. Нужны manifest approval, snapshot binding и различение pass/cancel/timeout; текущий `Report` для cancelled возвращает `passed=true`, что нельзя переносить в семантику `done` |
| `Sandbox: off/worktree/container` в `cmd/internal/agentrunner/sandbox.go` | Worktree не sandbox безопасности. Текущий container общий на workspace и монтирует workspace; это не изоляция control plane и не независимая среда каждого Experimenter |
| `Quota.Windows` в `cmd/internal/accounts/quota.go`, utilization, reset time, live/observed источники | Нет доказанной формулы расхода подписки или гарантии независимости общих и scoped-окон; нужны стабильные window IDs и история снимков |
| Live plugins, workspace trust, роли плагинов и skills | Нет per-run approval неизменяемого bundle и ограничений полномочий для agent-generated executable plugins |
| Heartbeat, compaction recovery, `.changes`-снимки, TranscriptSearch | Нет watchdog по цели, durable quota waiting и handoff цепочки с fencing |

В AUDIT: 22,2 ч, 57 компакций, 6 сегментов без действий, 74 чтения `gfxFont.cpp`, 39 повторных накатов
правок, 44 неверных имени тула; у задачи №4 — 20,2 ч без итогового отчёта, 22 неуспешных живых прогона.
Utilization подписочных окон **неизвестна**. Сессионные и шлюзовые token counters в AUDIT различаются;
их не складываем и не превращаем в точную оценку расхода подписки.

### 0.2. Термины

- **Goal** — подтверждённый контракт результата и ограничений. **Chain/run** — весь запуск по этому
  контракту, включая design, сессии Writer, помощников, review и reflect; rollover не обнуляет бюджет.
- **Session** — один контекст и процесс раннера. **Turn/ход** — один regular request/response основной
  модели; compaction, Judge и Reviewer имеют отдельные счётчики. **Segment** — интервал между компакциями
  или rollover; используется для диагностики, но не как единственные часы watchdog.
- **Candidate** — неизменяемый snapshot результата, предложенный к приёмке. **Evidence** — запись харнеса
  об операции/артефакте; объяснение модели помечается отдельно и не становится фактом от наличия timestamp.
- **Control plane** — контракт, разрешения, scheduler, budget, verify и их authoritative history.
  **Object plane** — код/документы задачи, исследования и эксперименты.
- **MUST** — инвариант режима; **default** — изменяемая пользователем настройка; числа без измерений —
  стартовые гипотезы для benchmark, не вывод о любом репозитории.

---

## 1. Принципы, границы доверия и контракт цели

1. **Один Writer lease на репозиторий.** Помощники исследуют и проверяют параллельно; проектные изменения
   принимаются последовательно. Worktree сам по себе не отменяет конфликты решений и владения.
2. **Один писатель authoritative state.** Coordinator/reducer принимает команды ролей; роли не правят
   общие runtime-файлы напрямую. Принятое событие фиксируется durably до подтверждения модели.
3. **«Сделано» — результат приёмки candidate, не слово в JSON.** Проверки, обязательный review и metric gates
   относятся к одной версии результата и контракта. Исторический pass не означает pass текущего дерева.
4. **Память переживает компакцию, но имеет provenance.** Goal — контракт, journal — наблюдения и решения,
   state — проекция. Саммари помогает переориентации, не заменяет ни один из них.
5. **Hard guards работают без LLM.** Stop/cancel, deadline, quota policy и доказательства завершения
   не зависят от доступности Judge или способности Writer написать финальный отчёт.
6. **Primary сильный; дешёвые роли — по проверяемому контракту.** Reviewer получает чистый контекст;
   Advisor — bounded fork контекста Writer. Модель/effort pinned на экземпляр роли.
7. **Цена — расход binding windows подписки.** Неизвестность отображается явно; резерв пользователя
   важнее продолжения автономки. Paid extra usage не включается автоматически.
8. **Самонастройка не повышает полномочия.** Модель меняет профиль только внутри заранее одобренного
   envelope. Изменение цели, gates, бюджетов, capabilities или кода харнеса требует control-input пользователя.
9. **Сначала простой вертикальный срез.** Для MVP не нужны distributed supervisor, vector DB или рой.
   Нужны корректное состояние, отчётность, quota pause и безопасная передача чистому контексту.

### 1.1. Что пользователь подтверждает до автономного запуска

`goal.md` — читаемая версия следующего структурированного контракта; машинные поля не извлекаются из
«первых N строк Markdown». Planner предлагает; харнес показывает полный diff; пользователь подтверждает
**digest конкретной версии**, из которой строится и Markdown, и policy.

| Часть контракта | Что фиксируется |
|---|---|
| Результат | Primary metric и источник измерения; baseline; quality gates; обязательные инварианты; условия, при которых human acceptance неизбежна |
| Приёмка | Approved verify-манифест, ожидаемый результат/парсер, fixtures/oracle, review policy; разрешённые исключения и кто их одобряет |
| Работа | Репозиторий/workspace, write-scope, допустимые внешние эффекты, режим isolation; политика checkpoint/commit |
| Бюджет | `max_active_hours`, абсолютный `deadline_at`, `max_compactions`, окна подписки (§3.1), расход всех ролей; отдельный подлимит уровня B |
| Остановка | Stuck thresholds по kind задачи; report interval; допустимые bounded recovery; политика quota unknown/weekly wait |
| Самонастройка | Каталог моделей, fallback-цепочки, допустимые параметры, tool/plugin capabilities и границы reflect |

Метрика может быть числом, набором acceptance checks или human-reviewed deliverable. Для research не
нужно притворяться, что качество открытия объективно измеряется количеством файлов. Если критерий
расплывчат, planner предлагает проверяемую формулировку и спрашивает пользователя; неподтверждённый
контракт остаётся `planned`, автономный run не стартует.

Смена контракта создаёт новую revision с control-event. Старые evidence не удаляются, но их применимость
к новым требованиям проверяется заново. «Interactive checkbox acceptable», новый oracle или разрешение
skip — предложение пользователя к решению, не поправка модели внутри саммари.

### 1.2. Что означает «вне досягаемости модели»

Различаем **целостность протокола** и **изоляцию ОС**. Схема JSON, запрет Write в tool list, worktree и
строка `[harness]` не являются security boundary при произвольном Bash на машине пользователя.

- В `trusted_local` защищаем от обычных ошибок: typed API, один reducer, immutable approved copies,
  revision checks и аудит. Честно сообщаем: shell с правами пользователя может обойти файловые запреты;
  runtime вне workspace уменьшает случайные повреждения, но не изолирует враждебный процесс.
- Для строгой недоступности control plane нужен `isolated` executor: отдельный процесс-контроллер,
  недоступный для записи runtime/approved manifests, ограниченные mounts/env/network, отсутствие
  Docker socket и доступа к host credentials. Применяется ко **всем** путям исполнения: Bash,
  Code/file tools и plugin commands, а не только к shell wrapper.
- Старт long-running режима показывает фактический isolation level и ограничения. Если контракт требует
  строгой защиты, а backend её не обеспечивает, run не стартует. Текущий container-режим нельзя молча
  рекламировать как такую защиту; до hardening доступны только его реальные гарантии.
- Сообщения модели, tool output, skills и плагины — недоверенные данные для control plane. Происхождение
  `user approval`/`harness evidence` берётся из доверенного канала, а не из текста или caller-supplied поля.

---

## 2. Состояние: три публичных файла и один authoritative store

### 2.1. Владение и хранение

`.harness/goals/<id>/` остаётся небольшим читаемым интерфейсом памяти. **Три файла не означают три
независимых источника истины.** Control plane хранит принятые события и approved versions в runtime вне
agent-write mounts; `<runtime>` ниже — выбираемый backend, не новый обязательный абсолютный путь.
В локальном MVP достаточно append-only store с одним сериализованным writer; SQLite — возможная
реализация, не требование. Переиспользуем sessionstore и его механизмы, где сохраняются инварианты,
не создаём второй конкурирующий лог тех же операций.

| Файл/данные | Кто предлагает | Кто принимает и пишет | Назначение |
|---|---|---|---|
| `goal.md` | пользователь / Planner | харнес после user approval | Читаемая проекция immutable approved contract; правила и ссылка на revision |
| `state.json` | Writer, Planner, помощники — только допустимые поля | reducer харнеса | Tasks, hypotheses, ledger, finding proposals; effective status и ссылки на facts |
| `journal.md` | модель — объяснения; харнес — факты операций | харнес, append events | Датированные записи с origin, evidence refs и пометкой «предложено / не подтверждено» |
| Verify, quota, approvals, leases, counters | только доверенные executors/control-input | authoritative store харнеса | Неизменяемые факты, из которых восстанавливаются проекции; модели доступны read-only views |
| Полный output/artifacts | executor | artifact storage | Ограниченное хранение, content hash, provenance; в контекст идёт digest и ссылка |

`verify_runs` в `state.json`, если показан, — **проекция read-only facts**. Подмена файла не подменяет
authoritative record. Харнес обнаруживает расхождение revision/digest, уведомляет модель и регенерирует
проекцию; не импортирует произвольный Write как факт. Для строгого режима сами проекции read-only.

Версионируем в Git контракт, редактируемые specs и пригодные к публикации checkpoint-проекции, если это
разрешено пользователем. Credentials, account IDs, raw outputs, quota history и runtime **не коммитим
по умолчанию**. Три файла могут содержать чувствительный контекст: redaction/политика публикации нужны
и для них. Git полезен как история результата, не как база транзакций scheduler.

### 2.2. Как роли изменяют состояние

Минимальный контракт: `GoalUpdate`, `JournalAppend`, `Verify`, `SubmitCandidate`, `HarnessStatus`.
Это проектируемые команды ядра, не произвольные executable plugins с правом менять meta-state.

Каждая мутация несёт `goal_id`, `expected_revision`, `request_id` и payload. Actor/capabilities харнес
определяет сам. Reducer последовательно проверяет schema, ownership, allowed transition и preconditions;
фиксирует event; возвращает новую revision. Повтор одного `request_id` возвращает прежний результат;
устаревшая revision — conflict с текущей дельтой, **не last-writer-wins**. Одновременные независимые
изменения можно батчить одной командой; конфликтующие сначала перечитать и согласовать.

- Writer/Planner изменяют задачи, гипотезы и proposed findings в разрешённом scope; status — запрос
  перехода, не присваивание. Нельзя удалять истории/пункты: допускаются superseded/withdrawn с причиной.
- Experimenter сдаёт приватный `result` и artifact refs. Харнес импортирует его в hypothesis result;
  прямой параллельной записи в общий JSON нет. Ошибочный результат остаётся с provenance.
- `JournalAppend` дедуплицируется по request ID. Авто-строки compaction/verify/rollover идут через тот же
  writer. Проекция Markdown обновляется атомарно; crash между event и проекцией лечится replay.
- `state.schema.json` расширяет только namespaced payload задачи; не переопределяет status, facts,
  capabilities или обязательные ограничения. Schema version и migration фиксируются событиями.
- Ошибка schema/диска возвращает явный отказ. Не подтверждаем принятие до durable записи; обрезанный
  хвост лога после crash восстанавливается только по предусмотренному store-протоколу.

Задача: `{id, kind, scope, depends_on, check_ids, acceptance_artifacts, attempt_id, status}`.
Гипотеза: `{id, claim, evidence_for, evidence_against, experiment, result_refs, status, deferred_reason,
next_probe, estimated_cost}`; `estimated_cost` — оценка модели с provenance, не measured quota fact.
DAG валидируется на циклы и отсутствующие dependencies; расширение scope/gates за envelope требует approval.

Task lifecycle: `pending → doing → ready_for_review → done`; findings возвращают в `doing` с новой attempt.
`blocked` требует reason, evidence последней попытки и предлагаемого unblock; не означает выполненную
задачу и не снимает её из goal gates. `skipped/withdrawn` принимаются только по approved skip policy,
иначе — запрос пользователю. Повторное открытие done сохраняет прошлую приёмку, не переписывает её.

### 2.3. Verify и приёмка точной версии результата

Команды не «распознаются в Bash» по тексту: похожий `go test`, `go test; true` или другой cwd не являются
одним и тем же approved check. Модель вызывает `Verify` по стабильным `check_ids`; харнес запускает
**approved immutable manifest**, скопированный в control plane при approval. Manifest фиксирует argv
или точный shell script, cwd, timeout, разрешённое окружение, required outputs и критерии результата.

Модель может предложить новые checks и добавлять проектные тесты. Ослабить approved gates, заменить
oracle/fixtures, использовать `KOU_CONVEYOR_VERIFY=off` или plugin verify как обход контракта нельзя.
Изменения test code входят в snapshot и diff Reviewer. Сам факт запуска изменяемого проектного теста
не исключает test tampering: критические fixtures/gates контролируются отдельно, остальные покрываются
review и итоговым integration verify.

Пример authoritative записи (значения идентификаторов иллюстративные):

```json
{
  "run_id": "run-234b",
  "task_id": "text-metrics",
  "attempt_id": "attempt-3",
  "candidate_id": "candidate-17",
  "snapshot_id": "snapshot-17",
  "contract_revision": 2,
  "manifest_digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
  "check_id": "build-and-smoke",
  "operation_id": "verify-29",
  "executor_origin": "harness",
  "started_at": "2026-10-02T10:00:00Z",
  "finished_at": "2026-10-02T10:01:20Z",
  "result_kind": "pass",
  "exit_code": 0,
  "timed_out": false,
  "output_digest": "sha256:2222222222222222222222222222222222222222222222222222222222222222",
  "artifact_ref": "artifact:verify-29"
}
```

Snapshot охватывает tracked и relevant untracked files, submodule revisions, fixtures и зависимости
проверки; `git HEAD`/mtime недостаточно. Generated/cache directories исключаются по approved policy,
только если не являются входом проверки. Environment/toolchain/image digest фиксируются отдельно.
Нельзя строить snapshot, пока Writer или другой процесс меняет его входы: нужен quiescence или
неизменяемая materialized copy. До/после проверки не должно появляться незаявленных входных мутаций.

`result_kind`: `pass | fail | cancelled | timed_out | inconclusive`. Pass требует завершённую операцию,
exit 0, отсутствие timeout/cancel и выполненные assertions. Cancel, невозможность запуска, исчерпание
числа проверок и «это ожидаемый fail» не становятся pass. Исключение требует отдельного approved waiver.
Хеш вывода доказывает целостность артефакта, **не семантическую корректность результата**.

Переход в `done` принимает харнес, если для одного candidate/attempt:

1. Все обязательные `check_ids` имеют применимый pass по approved manifest и contract revision.
2. Результат измерения удовлетворяет primary/quality gates; если нужна human acceptance, она получена.
3. Reviewer PASS существует, когда он обязателен policy, и относится к тому же snapshot/diff.
4. Нет несогласованных изменений входов после candidate, а dependencies удовлетворены.

Поздняя правка не удаляет исторический pass, но делает его **stale для нового candidate**. Перед
`chain.completed` обязательно проверяется финальный integration snapshot, а не сумма старых task passes.
Изменение shared dependency может потребовать reopen задач; пока impact-analysis нет, итоговый verify
покрывает весь approved integration gate. В MVP без Reviewer его отсутствие явно видно в policy —
не выдаём этот уровень приёмки за полный review-loop.

### 2.4. Контекст и сохранение отрицательного опыта

- На старте и после compaction харнес строит bounded view: approved goal, текущая задача/dependencies,
  открытые и релевантные refuted/deferred hypotheses, последний verify, tail journal и budget status.
  Обязательные правила не проходят через саммари и никогда не обрезаются «первыми N строками».
- Стартовые бюджеты context slots: contract ≤ 2k токенов, state digest ≤ 4k, journal tail ≤ 2k.
  Если контракт не помещается, нужен явный меньший digest с approval, не молчаливое удаление правил.
- Recitation: default каждые 20 regular turns или при смене задачи/важном verify — дайджест ≤ 1,5k
  токенов в конец истории. Динамический digest не переписывает статический system prefix. Точное
  сообщение и cache behavior выбираются по provider adapter; «system message в конце» поддерживают
  не все провайдеры. Эффект на cache hit rate измеряем, а не гарантируем.
- Перед порогом compaction — bounded checkpoint opportunity: принять state/journal, показать snapshot,
  затем compact. Default запас `min(30k, 15 % context_window)`; только один checkpoint-turn, без
  рекурсивного «ещё обнови память». При overflow/недоступной модели authoritative events уже сохранены.
- «Keep the wrong stuff in»: сохраняем error class, digest, причины refutation и artifact refs.
  Полный многомегабайтный failed output не pinned в контексте. После compaction ошибки доступны через
  facts/journal; релевантный отрицательный опыт возвращается в digest. Superseded гипотеза не теряется,
  но весь исторический реестр не вставляется каждый ход.
- Thinking signatures и provider-specific context management не переписываем произвольно.
  `clear_thinking`/server prune — capability adapter с тестами совместимости и измерением cache;
  profile выбирает только реально поддерживаемые настройки.

### 2.5. Checkpoints и пользовательские изменения

До старта фиксируются baseline, dirty/untracked inventory и выбранная commit policy. Default автономки —
отдельный chain worktree; начальное содержимое согласуется с пользователем: чистый HEAD или snapshot
с включением конкретных dirty files. Нельзя молча потерять незакоммиченные изменения при создании worktree.
Chain workspace сохраняется при rollover, не создаётся заново от HEAD для каждой новой session.

Коммиты — только если разрешены policy; не коммитим в `main`, не включаем чужие изменения, не делаем push
без отдельного разрешения. Без auto-commit используем harness snapshots и journal. В non-Git workspace
допустим snapshot backend; если доступен только worktree backend, честный отказ до запуска.
Внешнее изменение проверяемых файлов инвалидирует candidate и требует reconcile. Writer lease
координирует харнесы, но не блокирует человека в редакторе и не заменяет detection внешних изменений.

---

## 3. Цепочка сессий: quota, watchdog, восстановление и rollover

### 3.1. Бюджет = окна лимитов подписки

#### Наблюдения, а не формула провайдера

Cockpit умеет читать `five_hour`, `seven_day`, model-scoped weekly и другие поля Claude usage; имеются
live и observed данные. **Наличие `seven_day_opus` не доказывает отдельный Opus 5h-бакет или отсутствие
общего weekly-лимита.** Не hardcode-им «Max = независимые окна», «Haiku = Sonnet bucket» или количество
доступных часов из оценок сообщества. Это provider/plan capabilities с provenance и сроком актуальности.

Нормализованный snapshot: `{provider, account_ref, window_id, scope, epoch_id, used_percent, reached,
resets_at, observed_at, source, freshness, error}`. `window_id` — стабильный ключ, не UI label;
`account_ref` — непрозрачный ID без credentials. Model mapping — множество binding windows с
`confirmed | inferred | unknown`. При неизвестном mapping нельзя обещать обход Opus/shared budget.

Один запрос может тратить несколько окон. Перед dispatch все применимые ограничения MUST разрешать
запрос. Разные аккаунты/провайдеры не складываются в «остаток 170 %»; показываем вектор окон и eligibility
аккаунтов. Quota общая для всех chain и интерактивной работы, поэтому scheduling/reservations принадлежат
общему account controller, не независимому экземпляру run.

#### Семантика policy

| Поле | Единица и действие |
|---|---|
| `reserve_percent` | Процент **всей ёмкости** окна, сохраняемый пользователю: utilization после автономного dispatch должна оставаться ниже `100 - reserve_percent` |
| `max_additional_pp` | Максимальная прибавка utilization в **процентных пунктах** этого epoch относительно baseline run; не доля оставшегося и не точная per-run цена |
| `max_active_hours` | Суммарное время, когда хотя бы одна роль/операция chain работает; параллельные роли не умножают wall clock. Design/review/reflect входят; quota/user waiting без работы не входит |
| `deadline_at` | Абсолютный UTC deadline, действует и во время ожидания; rollover/restart его не продлевают |
| `max_compactions` | Все compaction attempts chain, включая неуспешные; детализация по ролям хранится отдельно |
| `quota_max_age_seconds` | Максимальная давность снимка для dispatch; default 900 с с refresh у границы лимита |
| `unknown_quota_action` | Default `pause_and_ask`; ограниченный grace возможен только в approved contract с отдельным временным лимитом |

Для применимого окна conservative utilization ceiling — минимум из `100 - reserve_percent` и
`baseline_used_percent + max_additional_pp` (если последнее задано). Добавляем measured safety margin
на in-flight requests и задержку quota API; нет гарантии точного enforcement провайдерской utilization,
когда данные запаздывают или пользователь расходует тот же аккаунт вне контроллера.

Пример approved budget policy — **предложение**, пока пользователь его не подтвердил:

```json
{
  "max_active_hours": 4,
  "deadline_at": "2026-10-03T06:00:00Z",
  "max_compactions": 20,
  "report_every_active_minutes": 30,
  "usage": {
    "quota_max_age_seconds": 900,
    "unknown_quota_action": "pause_and_ask",
    "windows": [
      {"selector": "claude:shared:5h", "reserve_percent": 15},
      {"selector": "claude:opus:weekly", "reserve_percent": 40, "max_additional_pp": 30}
    ],
    "on_limit": {"short": "pause_until_reset", "weekly": "report_and_ask"}
  },
  "harness_change": {"max_active_hours": 1, "max_additional_pp": 10}
}
```

Selectors в примере — proposed normalization, не ключи существующего API. Если окно не найдено или
mapping не подтверждён, это `quota_unknown`, а не «условие выполнено». Подлимит уровня B расходуется
**внутри** общего approved budget, не даёт бесплатных дополнительных часов/окон.

#### Телеметрия и атрибуция

Account service пишет quota history, даже когда cockpit закрыт: default каждые 10–15 мин при активных
chain; дополнительно старт, verify/task boundary, compaction, rollover, limit и finish. Не делаем refresh
каждым Judge; используем существующие dedup/cache и rate limit самого quota endpoint.

Utilization delta — **account observation**, не автоматически расход данной роли. Записываем overlap
других сессий, confidence и принадлежность запросов аккаунту, если шлюз её сообщает. Для модели/роли
показываем measured/estimated/unknown drain и диапазон, не ложную точность «пунктов на файл».

Не вычитаем utilization через reset, смену плана, account rotation или несовместимые snapshots.
Для скользящего окна падение usage может быть expiry старых запросов: clamp к нулю не восстанавливает
истинный расход. Изолированные интервалы можно использовать для оценки, остальные — только для текущих
ceiling и отчёта «общая utilization изменилась». Reset создаёт новый epoch; baseline обновляется событием,
а расход прошлых epoch, active time и deadline остаются в истории. Budget across multiple epochs
задаётся явно; reset не является неявным разрешением тратить бесконечную цепочку.

#### Пауза вместо смерти

Вместо повторных `Failure` до `maxFailures` адаптер возвращает нормализованный rate-limit signal:
`{kind: subscription | transient | unknown, account_ref, binding_window_ids, retry_at, evidence_ref}`.
429 может означать кратковременный RPM или перегрузку, а `cooling` — сетевой сбой; сами по себе они не
доказывают исчерпание подписки. Краткие retry остаются bounded транспортным механизмом; подтверждённое
долгое ожидание становится состоянием chain.

1. При подтверждённом short-window limit сохраняем `waiting_for_quota`, blocked roles, reason,
   `resume_at`, attempt counter и deadline. Судья/Writer не вызываются; local timers, прием control-input,
   сбор результатов существующих tools, cancel и deterministic report продолжают работать.
2. `resume_at` учитывает **все** binding limits выбранного маршрута; ближайший reset одного из них
   недостаточен. Если есть другой разрешённый eligible account, controller может выбрать его по policy.
   Если reset неизвестен — bounded refresh/backoff, затем `awaiting_user`, не бесконечный retry.
3. При наступлении срока сначала refresh и reconcile контракт/workspace, затем dispatch. Resume event
   идемпотентен; модель получает утверждённый короткий resume prompt и актуальный state digest.
4. Default не более двух автоматических re-arm без успешного regular response/полезного прогресса;
   повторное немедленное исчерпание → `awaiting_user` с отчётом. Счётчик сохраняется после restart.
5. Weekly wait на дни требует approval с новым/совместимым deadline. Default — сохранить checkpoint,
   объяснить блокировку, ждать решения, не держать живой runner дни без необходимости.
6. Scheduler сохраняет ожидание вне процесса сессии и после restart заново вычисляет timer из UTC.
   Отмена/истёкший deadline приоритетнее resume; недоступность модели не мешает остановить chain.

Fallback: текущую модель не подменяем. Другой аккаунт той же модели допустим с явной записью и учетом
provider/session constraints; другая модель/effort — новый экземпляр роли/новая session после checkpoint,
если approved fallback разрешён. При незавершённых mutating operations сначала quiescence.

### 3.2. Watchdog и отчётность — до LLM Judge

Hard guards: cancel, deadline, active budget, quota policy, verify/review gates и task dependencies.
Они исполняются перед dispatch и при каждом значимом событии; counters — на chain, не на context.

Progress — не факт любого tool use и не количество строк кода:

| Kind | Что считается новым наблюдаемым продвижением |
|---|---|
| `feature` | Новый candidate diff, завершённый релевантный verify/live-run, принятая гипотеза с evidence, приёмка задачи |
| `experiment` | Завершённая проба с inputs/version/result refs; отрицательный результат тоже progress, если не повтор без нового входа |
| `research` | Принятый artifact/ledger entry с привязкой к file snapshot/range/citations; новый evidence-backed finding или ответ на research question |

Эти события доказывают выполнение работы, не истинность всех выводов. Quality проверяется gates/Reviewer.
Повторное чтение, журнал «ещё исследую», пустой Edit, новый timestamp прежнего результата или перезапуск
одной пробы не сбрасывают stuck detector автоматически. Ledger с самим собой подписанным `done` —
не независимое доказательство, что файл качественно изучен.

Стартовые defaults для feature: soft warning после 20 active minutes без progress, hard checkpoint после
60 min или 3 segments без progress — что раньше. Research/experiment имеют свои approved timeboxes;
нет обязательного «написать код» для research. Действия после soft warning: сформулировать next probe,
сохранить evidence, один bounded reorientation. После hard threshold — report и `awaiting_user/stopped`
по policy; не бесконечная последовательность rollover с новой надеждой.

Отчёт пользователю default каждые 30 active minutes, на task boundary, limit, stuck и перед stop:
primary metric/baseline, last verified snapshot, что подтвердилось/опроверглось, blocker, следующий шаг,
оставшиеся часы и quota freshness. Если LLM недоступна/бюджет исчерпан, харнес строит такой отчёт из
facts сам. Финальный model turn возможен только в заранее зарезервированном budget и с timeout.

Для running tools check-in через 30 мин → 1 ч → 2 ч, максимум 3 для одного unchanged operation set;
это отдельный таймер, не замена report interval. Сначала проверяем состояние/новый output детерминированно;
LLM зовём только при необходимости решения. Tool deadline/cancel policy действует и после последнего
check-in. Не запускаем `sleep`-поллинг и новые пробы, пока старая mutating операция не завершилась.

### 3.3. LLM Judge: дешёвый советник, не источник фактов

Default в autonomous mode — после завершённого regular Writer turn, **когда его relevant tool results
уже доступны**; не оцениваем pending tools как отсутствие прогресса. Input ограничен goal digest,
новой дельтой, актуальными facts/counters и metric; полный многотысячный transcript каждый раз не шлём.
Без тулов и прав изменения состояния. Возможна настройка every-N turns по measured overhead.

```json
{
  "decision": "not_yet",
  "evidence_refs": ["verify-29"],
  "next_action_class": "reorient",
  "hint": "Проверь следующую открытую гипотезу из state.",
  "confidence": 0.7
}
```

Enum decision: `met | not_yet | impossible`; next action: `continue | reorient | ask_user | stop`.
Response валидируется schema, размером и ссылками на существующие evidence. `met` **не обходит §2.3**;
`impossible` означает предложенную блокировку/остановку с причиной, не доказательство невозможности.
Hint — недоверенный advisory input; не может отменить approved goal или дать новые полномочия.

Default timeout 15 с, не более одного pending Judge на role; результат для старой revision не применяется
к новой. Timeout/invalid output дают event и bounded fail-open **только для advisory оценки**, при действующих
hard guards. Если контракт требует Judge, после двух подряд сбоев — `awaiting_user`; бесконечных retry нет.
Judge не находится на обязательном critical path tool delivery. Measure calls/latency/drain отдельно:
утверждение v2 «доли процента» — гипотеза, не принятая стоимость. User-visible reports работают без Judge.

### 3.4. Машина состояний chain и приоритеты переходов

Отделяем состояние chain от activity внутри running session, чтобы не дублировать все tool phases.
`activity`: `writing | waiting_tools | verifying | reviewing | compacting | reflecting`.

| Chain state | Условие входа | Что разрешено / как выйти |
|---|---|---|
| `planned` | Контракт предложен, но не approved | Только design/approval; approved version + проверенный workspace → `running` |
| `running` | Есть действующий контракт и controller owner | Dispatch в пределах guards; завершение gates → `completed`; quota/user/handoff/recovery — по событиям |
| `waiting_for_quota` | Нет разрешённого маршрута для очередного dispatch | Никаких расходующих blocked quota LLM requests; tool completion/cancel/control/timer доступны; refresh → `running` или `awaiting_user` |
| `awaiting_user` | Нужны approval, уточнение, принятие blocker или unsafe recovery | Новые autonomous side effects запрещены; существующие операции завершаются/отменяются по policy; user control → reconcile и продолжение либо stop |
| `handoff_pending` | Запрошен rollover | Старый Writer не получает новых ходов; drain/cancel операций; durable handoff → следующий owner |
| `recovering` | Restart/crash/неопределённый исход операции | Восстановить events, guards, leases и operations; никакого speculative второго Writer; reconcile → прежнее waiting или `running` |
| `completed` | Финальный snapshot прошёл все goal gates | Terminal, успешный отчёт; продолжение только новым approved run |
| `stopped` | Budget/stuck/user soft stop без выполненной цели | Terminal, частичный результат и причина; это не success и не infrastructure error |
| `failed` | Неустранимая runtime/store/verification-infrastructure проблема | Terminal с checkpoint/recovery instructions; task test fail обычно не делает весь run failed |
| `cancelled` | User hard cancel | Terminal после прекращения управляемых side effects и учёта неизвестных исходов; не pass |

Приоритет: hard cancel → deadline/hard budget → integrity/recovery → user control → quota → review/task
scheduler → advisory Judge. Late tool/Judge result не возрождает terminal chain; записывается как факт
с привязкой к исходной attempt. Soft stop не должен ждать неделями квоты ради финальной LLM-фразы.

### 3.5. Durability, leases и границы «ровно один раз»

Минимальная runtime-запись: `{chain_id, schema_version, contract_revision, profile_revision, state,
activity, active_session_id, workspace_id, writer_epoch, counters, deadline_at, pending_intents,
resume_at, last_event_sequence}`. Все изменяющие её события сериализованы одним controller.

- Local MVP: OS lock/controller owner на canonical repo ID (для Git — common repository identity,
  а не только path worktree), плюс монотонный Writer epoch. Не запускаем новый Writer, пока предыдущий
  owner не остановлен/не доказано мёртв. TTL/пропущенный heartbeat сами по себе не дают право на второй Writer.
- Все dispatch/accept commands проверяют epoch. Уже запущенный произвольный shell не остановить одним
  fencing number: supervisor должен завершить его процессную группу либо подтвердить исход. Поэтому
  при неизвестном старом процессе — recovery/pause, а не «lease истёк, безопасно продолжать».
- Intent и стабильный operation ID сохраняются до dispatch; terminal result сохраняется до следующего
  зависимого решения. Дубликат intent/result дедуплицируется. Применяем существующий operation recovery,
  но отдельно проверяем, где его idempotency действует только в памяти одного manager.
- Не обещаем exactly-once внешних эффектов. Для shell/HTTP/deploy с неизвестным исходом нужны idempotency
  key внешней системы или reconcile; иначе `inconclusive/awaiting_user`. Не повторяем mutating command
  автоматически только потому, что потерялся ответ. Retry policy задаёт safe-to-repeat класс операции.
- После restart восстанавливаем projected state, deadline, quota age, attempts и waiting timers из store.
  Несовместимая schema/binary version даёт явный отказ или approved migration, не тихое игнорирование.
- Artifact/event retention не удаляет evidence активного run. Диск-full останавливает новые side effects;
  privacy/redaction проводится до context/export, raw artifacts имеют отдельный доступ и retention.

### 3.6. Rollover: чистый контекст без нового результата «из воздуха»

| Сигнал | Действие |
|---|---|
| Задача accepted/blocked, смена направления | Рассмотреть rollover на boundary; blocked сохраняется в контракте, не скрывается |
| Soft stuck/repeated reads без novelty | Bounded reorientation; rollover не чаще лимита policy и не сбрасывает watchdog |
| `rollover.every_compactions` достигнут | Backstop: default 10, проверяется benchmark; значение 40 из чужого кейса не универсальная норма |
| Контекст у порога посреди задачи/есть running tools | Compaction; или отложенный rollover после quiescence |
| Hard stuck/budget/deadline | Stop/ask с отчётом, не ещё один rollover |

**MVP: никаких running operations через rollover.** Сначала stop dispatch, завершить или отменить
операции с известным исходом, сохранить candidate/state/journal. Quota pause не заставляет делать
rollover, если tools ещё работают. Если drain не завершился в approved timeout, сохраняем исходный
owner и сообщаем blocker. Перенос живых операций допускается только отдельной реализацией durable
supervisor с тестами ownership и recovery; наличие operation ID недостаточно.

Протокол handoff:

1. Persist `handoff_requested` с причиной, counter и current epoch; запретить новые Writer turns.
2. После quiescence persist immutable пакет: goal/profile revisions и digest, state revision,
   chain workspace/snapshot/dirty inventory, краткий git log/diff-stat, last verify/review refs,
   открытые hypotheses, quota/budget/counters, **одна следующая задача**. Полный контекст не передаём.
3. Создать child session в неактивном состоянии по стабильному `handoff_id`; записать связь parent/child.
   Повтор после crash находит тот же child, не создаёт ещё один. Parent не получает новых dispatch.
4. Завершить старый runner/Writer owner, затем передать lease/epoch и активировать child. Между шагами
   может не быть Writer, но никогда не должно быть двух; crash восстанавливает фазу по событиям.
5. Child проверяет workspace и handoff revision, делает approved smoke-verify, затем следующую задачу.
   Несовпадение → `recovering`, не работа по stale handoff. Checkpoint/commit — по §2.5.

Lineage — chain ID, parent/child, причина, model/profile/binary versions. Для нового контекста лучше явная
parent-ссылка с bounded handoff; `ItemFork` не должен незаметно вернуть полный родительский контекст.
`TranscriptSearch` по lineage — новое расширение поиска, не обещание существующего `--lineage` flag.

---

## 4. Навигация по коду

Из 234b видно две разные проблемы: повторное получение уже известных сведений (память/watchdog §2–3)
и неудобная навигация по большим файлам. Индекс не лечит забывание сам по себе.

1. **Окно Read по умолчанию**: первые ~200 строк + bounded outline/подсказка; дополнительные лимиты по
   bytes/tokens; полный файл — явный запрос в рамках output budget. Точные defaults проверяем на типах
   задач; эффект из чужого benchmark с окнами ≤50 строк нельзя приписывать нашим 200 строкам заранее.
2. **`Outline path` на tree-sitter**: symbols/ranges, language support, parser version и source hash.
   `Read symbol` возвращает bounded диапазон; fallback для неподдерживаемых/битых файлов — обычный Read.
3. **LSP** лениво по языкам: definitions, references, diagnostics. clangd/rust-analyzer требуют реальной
   конфигурации проекта; сервер может быть дорогим/недоступным. `replace_symbol_body` — отдельная mutating
   capability Writer, не read-only research tool; применяется с source-version check.
4. **Read ledger** на chain: path + snapshot/range + source hash + role/turn. 74 чтения одного пути — smell,
   не доказательство забывания: файл мог меняться. Повторы неизменных диапазонов без нового результата
   вызывают предложение записать вывод, а не автоматический запрет чтения.
5. **Repo map** bounded по текущему task scope; обновление при изменениях, source revision видна.
6. **Embeddings** — плагин после базовой навигации; opt-in с privacy/index freshness policy. Метрики:
   relevant retrieval, время до verified progress, повторные чтения, quota drain. «Прочитал → Edit» —
   proxy и плохо подходит research/review; не единственный критерий качества поиска.

---

## 5. Роли: один писатель и помощники; plan-режим

### 5.1. Capability matrix

| Роль | Контекст | Запись/исполнение | Момент запуска |
|---|---|---|---|
| Writer | Goal + state + собственная история | Project write-scope под lease; GoalUpdate/JournalAppend; approved tools/verify | Основная работа; сильная модель |
| Judge | Bounded delta/facts, без Writer thinking | Никаких тулов или state writes; advisory JSON | §3.3 |
| Reviewer | **Чистый**: контракт, diff от baseline, exact candidate, check manifest/results | Read exact snapshot, independent verify в отдельной copy/sandbox; findings через controller | Перед обязательной task/goal acceptance |
| Advisor | Bounded fork Writer, с artifact refs вместо oversized output | Советы; read-only tools при необходимости | По запросу или approved escalation policy |
| Researcher | Отдельный scoped контекст | Read-only source snapshot, private result artifacts | Параллельно; summary ≤2k токенов + refs |
| Experimenter | Hypothesis + immutable input snapshot | Пишет и запускает только в disposable copy, возвращает result/evidence | Параллельно до MaxWorkers |
| Planner/Design | Goal, manifest, bounded repo view | Только plan/spec proposals в staging; не код задачи | До run или approved replanning |
| Reflect | Телеметрия/facts в чистом контексте | Proposed memory/profile delta; reducer принимает внутри envelope | На boundary и финише с отдельным timebox |

«Без Write/Edit» не делает роль read-only при arbitrary Bash. Read-only роли получают allowlisted
read tools; verify исполняется отдельным executor. Тесты Reviewer могут писать cache/generated files,
поэтому запускаются не в активном Writer workspace. Experimenters могут править экспериментальную
копию, но не merge, shared control state, approvals или authoritative facts. Текущий общий container
на workspace не выдаём за независимые sandbox-ы этих ролей.

Reviewer не получает reasoning/обоснование Writer до независимого вывода, но получает требования,
baseline и known constraints: «чистый» не означает «слепой». Verdict структурирован:
`PASS | NEEDS_WORK[severity, location, reproduction, evidence_refs] | INCONCLUSIVE`; относится к
candidate ID. Writer может оспорить finding с evidence; обязательный gate не обходится игнорированием.
Default максимум 2 fix-review цикла, затем user report/решение; лимит — параметр контракта.

Другой frontier Advisor полезен как capability-router, но полный fork стоит quota/context и может
содержать секреты. Используем только approved providers, bounded input и отдельный subbudget.
Не обещаем одинаковый эффект «2 бага/PR» для нашего проекта без локального измерения.

### 5.2. Scheduler и parallel writers

Planner предлагает DAG с kind/scope/dependencies/check IDs; reducer валидирует его. Research и experiments
параллельны в независимых copies; feature — один Writer. Result помощника — evidence-backed artifact,
не команда немедленно править общий файл. На task boundary — report и review по policy.

`parallel_writers: true` — поздний opt-in с явным ослаблением single-writer default: доказуемо независимые
scope на уровне пакетов/крейтов, отдельные branches/worktrees и leases. Manager-merge имеет один lease,
ребейзит/интегрирует последовательно; итоговый snapshot заново проходит integration verify и review.
Непересечение путей не доказывает независимость API/dependencies. Нет безопасного доказательства —
последовательный режим. Reviewer/Experimenter не считается parallel Writer, пока не пишет принятую
проектную ветку.

---

## 6. «Сырой поток»: design → run → reflect → архив и штатная эскалация

### 6.1. Смысл и границы

Модель под задачу выбирает протокол: ledger, роли, навигацию, checkpoint/rollover, экспериментальные
контракты. Для этого обычно достаточно конфигурации и task-local tools. Идеи ADAS/DGM, sleep-time,
dreaming и skills из опыта полезны как ориентиры, но не означают право модели менять свою fitness
функцию, права или рабочий binary. Внешние результаты исследований не являются гарантией качества
данного харнеса.

Нужно различать **proposed specification** и **effective approved specification**. Design-файл в Git
не является control-input сам по себе. Reflect может менять только параметры внутри утверждённого
envelope; любое расширение прав/ресурсов или ослабление gates остаётся предложением до approval.

### 6.2. Вход design

- **Capability manifest**: реально поддерживаемые profile fields/ranges, tools и isolation levels,
  роли, checkpoint/verify/rollover APIs, возможности hot reload и лестница исправлений (§6.6).
- **Model catalog**: actual IDs/context/effort/capabilities; eligibility/cooling; binding windows и
  confidence mapping; observed/estimated drain, latency и sample size. Human notes отделены от измерений.
- **Bounded repository view**: tree/размеры/языки, README/AGENTS, baseline, dirty state, build/verify,
  repo map текущего scope; не читать весь код харнеса при каждой задаче.
- **Archive**: несколько сравнимых прогонов и последний, с metric/gates, repo/environment/profile versions,
  quota confidence и outcome. Несравнимые задачи не ранжируются одним coverage score.
- **Workspace memory**: факты/практики с source refs, наблюдением по времени и условиями применимости;
  отдельно известные `harness_findings` и workaround-ы.

`HarnessStatus` показывает effective contract/profile revision, isolation, Writer lease, state/activity,
running operations, last verify/review, quotas/freshness, budget/counters, next timer и proposals awaiting
approval. Не выводит credentials и приватные идентификаторы других аккаунтов.

### 6.3. Выход design и approved profile

Staging в `.harness/goals/<id>/harness/`:

```text
profile.json        proposed role/model/effort/tool choices и bounded context/rollover параметры
plugin/             черновик task-local plugin bundle; не auto-discovery каталог
state.schema.json   namespaced task payload, без переопределения meta-полей
prompts/            role/compaction additions; обязательные sections не удаляются
proposals/          запросы approval/уровня B; тексты не дают разрешение сами по себе
```

Реальная workspace plugin discovery идёт из `.harness/plugins/<name>/`, не из goal staging.
После approval controller активирует **immutable approved bundle**. Для long-running нужны per-run
pinning/approval дополнительно к существующему workspace trust; простого копирования изменяемого draft
в trusted plugin directory недостаточно (§6.5).

Пример execution profile для аудита (catalog aliases разрешаются в actual model ID/effort и фиксируются
при запуске роли; это не существующие имена API):

```json
{
  "schema_version": 1,
  "approved_contract_id": "repo-audit-v1",
  "roles": {
    "writer": {"model_ref": "strong_default", "capability_set": "project_writer"},
    "scan": {"model_ref": "bounded_scan", "capability_set": "snapshot_reader", "parallel": 4, "batch_files": 5},
    "analyze": {"model_ref": "strong_default", "capability_set": "snapshot_reader"},
    "judge": {"model_ref": "bounded_judge", "capability_set": "no_tools", "every_regular_turns": 1},
    "reviewer": {"model_ref": "strong_review", "capability_set": "candidate_review"}
  },
  "context": {"read_window_lines": 200, "recitation_every_regular_turns": 20},
  "rollover": {"every_compactions": 10, "on_task_boundary": true},
  "research": {"artifact_namespace": "file_ledger", "max_unreviewed_entries": 50}
}
```

Metric/budget/verify/judge acceptance rules находятся в approved contract, не дублируются изменяемыми
полями profile. Для ledger-аудита контракт задаёт frozen inventory с file hashes, исключения и skip policy,
coverage как primary, finding/citation quality и review sample как gates. Сканер сдаёт artifact на файл;
harness timestamp подтверждает приём артефакта, **не доказывает осмысленное чтение**. Выборка независимого
Reviewer проверяет качество; превышение skip policy требует решения пользователя. Смена исходного файла
делает относящийся к нему audit artifact stale по правилам контракта.

Scan role на более доступном окне может сберечь scarce quota, только если mapping/измерения это
подтверждают. Числа «5 файлов/ход» и «4 роли» — стартовые параметры, сравниваем с сильным single-agent
baseline по качеству, active time и binding-window usage; не обещаем заранее ускорение относительно чужих 36 ч.

### 6.4. Фазы и stop policy

| Фаза | Кто/права | Стартовый timebox | Выход |
|---|---|---|---|
| 0. Контракт | Planner + пользователь | До autonomous run | Approved metric/gates/budget/envelope и baseline; иначе `planned` |
| 1. Design | Сильная модель; proposals only | ≤30 active min, ≤1 compaction | Profile/spec delta, обоснование, при необходимости level B proposal; approval конкретного digest |
| 2. Run | Chain по §2–5 | Approved total budget | Candidates, evidence, accepted/blocked tasks и отчёты; harness findings по ходу |
| 3. Reflect | Отдельный агент, чистый контекст | ≤5 active min на boundary | Proposed profile delta, curated memory, comparable archive outcome и findings; не подмена criterion |

Если design timebox исчерпан: запуск approved default profile возможен только при уже подтверждённом
контракте и допустимом default; иначе ask/stop. «Нужно ещё изучить» не открывает дополнительные 7 часов.
Reflect не должен блокировать срочное quota pause/cancel; при недоступной модели архивируется factual
outcome, а курация откладывается. Все роли входят в общий budget; их лимиты не добавляются поверх него.

### 6.5. Ограждения профиля, плагинов и памяти

**Неизменяемое моделью:** контракт/бюджет, acceptance rules, approved verify/oracle, authoritative evidence,
telemetry, approvals, Reviewer requirements, Writer leases и policy executable plugins. Reflect меняет
model/effort/batch/context только в envelope и на безопасной boundary; уже запущенная роль pinned.
Ужесточать stop/rollover можно в разрешённых пределах; ослаблять только через новый user control.
Все revisions — с причиной и telemetry refs; самодекларированное «стало лучше» недостаточно.

**Плагины:**

- Plain skill/instruction текст предпочитаем executable plugin, если достаточно текста. Он всё равно
  недоверенный input и не может отменить core policy; system-wide scope модель без approval не меняет.
- Approval привязан к content digest manifest **и всех** scripts/prompts/web assets/dependencies,
  capabilities, scope, env/network/mounts. Пользователь может заранее одобрить узкий capability envelope;
  не обязательно спрашивать за каждый текстовый tweak внутри него, но права не расширяются молча.
- Agent-generated bundles не заменяют `core`, approval UI, verify, quota controller или чужие tool names.
  Для активного autonomous run новые executable/web capabilities staged до approval; «видно в cockpit»
  не равно разрешению. Подмена файлов/symlink после approval не меняет pinned bundle.
- `Bash:ro` — не декларация безопасности: allowlisted commands либо изолированный executor. Path traversal,
  symlink escape, env/secret access, network и shared mounts проверяются backend, не инструкцией модели.
  Для trusted_local явно остаются ограничения §1.2.
- Runtime approval API не доверяет workspace web script. UI только показывает/отправляет намерение;
  backend проверяет user origin, exact digest и envelope. Исполняемые плагины не выдают сами себе approval.

**Память/архив:** `.harness/memory/` и archive — публичные curated views, не raw runtime dumps.
Запись памяти содержит `{claim, source_refs, observed_at, repo_revision, environment, confidence,
expires_or_recheck}`. Отделяем measured facts от model interpretation. «Сборка 20–45 с» без toolchain,
commit и даты не превращается в вечный факт. Reflect предлагает merge/dedup/supersede; исходные evidence
не удаляет. TTL/токенный бюджет, revalidation и privacy policy не дают памяти стать новым 88-КБ summary.
Данные из внешних docs или прошлого run не повышаются до инструкций control plane.

### 6.6. Лестница исправлений: конфиг → плагин → код

Модель должна уметь заметить недостаток харнеса и запросить исправление, а не держать workaround
17 часов. Телеметрия предлагает **smell**, не объявляет доказанный bug.

| Smell | Evidence/проверка | Почему одного счётчика мало |
|---|---|---|
| Потеря Edit | Повтор path/old_string после успеха; before/after hashes; минимальный concurrent replay | Повтор после revert/external edit может быть нормальным |
| Неверное имя тула | Неизвестное имя, зарегистрированный adapter mapping, repeated errors | Arbitrary suffix matching может вызвать не тот привилегированный tool |
| Повторные чтения | Unchanged source hash/range, chain counters, отсутствие нового artifact | Изменяющийся файл законно перечитывается |
| Limit death | Нормализованный rate-limit signal, binding window, terminal reason | Не всякий 429/cooling — subscription limit |
| Compaction overflow | Request bytes/image counts/token estimate, adapter error | Нужны payload limits, не только больше compactions |
| Нет progress | Kind-specific evidence, active time и loop signature | Число Edit не измеряет research |
| Медленный цикл | Duration конкретного verify/build и comparable baseline; polling pattern | Время из старой машины/commit не обязательно релевантно |

Finding: `{id, kind, evidence_refs, impact_on_goal, workaround, rungs_tried, proposed_rung, status}`;
status `open | proposed | approved | in_progress | fixed | declined | superseded`. Model interpretation
отделена от observations, детектор не читает/сохраняет hidden thinking как обязательный источник фактов.

| Ступень | Изменение | Разрешение |
|---|---|---|
| 1. Конфиг | Поддерживаемые thresholds, batch, context, роли, более строгий stop | Самостоятельно внутри envelope; иначе proposal |
| 2. Плагин/skill | Task-local навигация, probes, instructions | По §6.5, с pinning и capability approval; не получает право менять core evidence |
| 3. Код, уровень B | Ядро операций/контекста, adapters, coordinator, cockpit | Явный user approval, отдельная `harness_change` session и review |

Ступень испробована или обоснованно отвергнута; не обязательно тратить бюджет на заведомо неприменимый
плагин. Workaround «один Edit на файл за ход» допустим временно, но не считается исправлением fileops.
`Verify` plugin может запускать диагностическую команду, но без trusted core integration его отчёт
не становится approved harness evidence. Alias normalization — adapter allowlist с отказом при ambiguity,
не «любое имя, заканчивающееся на Write».

**Протокол уровня B:**

1. **Proposal.** Draft `harness/proposals/<id>.md`: дефект/evidence, почему 1–2 не хватает, target repo/base,
   scope и protected surfaces, expected effect, падающий воспроизводящий тест, regression/replay plan,
   time/quota subbudget, isolation и rollback. Controller создаёт карточку, но файл не является approval.
   Основная chain продолжает безопасный workaround; если его нет — pause с сохранённым состоянием.
2. **Approval.** User control: Approve / Approve with limits / Decline / Later. Approval связан с digest,
   scope, base revision, capabilities, budget и сроком. Изменённый proposal требует нового approval.
   Protected surfaces: permissions/control-input, budget/quotas, verify/evidence, telemetry/status,
   Judge/Reviewer policy, plugin loader и executor isolation. Их изменение требует дополнительного
   явного разрешения **до соответствующих правок**, а не только замечания Reviewer постфактум.
3. **Execution.** Отдельная `kind: harness_change` session, pinned strong model, worktree настроенного
   `harness_repo`, approved branch и scope, TDD. Один активный level B Writer. Если основная задача тоже
   пишет репозиторий харнеса, она отдаёт lease/ставится в очередь: отдельный worktree не отменяет правило.
   Tests/replay запускаются из trusted controller; candidate code не подменяет evaluator текущего run.
4. **Verification.** Целевой failing→passing test, `go build ./...`, `go test ./...`, соответствующие
   race/fault/replay проверки; `benchmarks/harbor` — когда релевантен. Manifest и benchmark fixtures
   frozen вне write-scope candidate. Exit/results/metrics фиксирует контрольный executor, не лог агента.
   Не запускаем переписанный раннер как доверенный evaluator самого себя.
5. **Review.** Чистый Reviewer по exact snapshot, approval/scope и regression evidence; out-of-scope или
   новые protected changes → stop/re-approval. PASS не означает разрешение на deploy. Для protected
   surfaces в финальном отчёте отдельно перечисляются влияния на trust boundary.
6. **Acceptance/deployment.** Пользователь принимает **конкретный reviewed commit/digest**, мержит и
   разрешает rebuild. Не automatic merge и не подмена binary работающего runner. Следующая quiescent
   session может использовать новый approved binary только после проверки version/store compatibility.
   Cockpit rebuild policy также согласуется: нельзя рассчитывать, что live Go rebuild обойдёт active runs.
   Если deployment невозможен на boundary, chain остаётся на старой версии с workaround.
7. **Memory.** `fixed` ставится после принятия исправления и verification на реально используемой версии,
   с commit/binary/date. «Ветка готова» — ещё не fixed для рабочего run. Archive сохраняет before/after и
   ссылку на rollout; rollback — пользователь возвращает прежний approved binary/version без стирания evidence.

Пример 234b: concurrent Edit smell → временная сериализация вызовов → минимальный replay → proposal
на per-path serialization и source-version check. Approve разрешает работу, не merge. Параллельные
независимые Edit должны сохраниться; конфликтующие edits обязаны дать явный conflict, а не ложный успех.
Аналогично оформляются quota pause, tool-name mapping и compaction byte limits. Отказ/Later не приводит
к новой карточке каждый rollover: re-propose только с новыми evidence или значимым изменением impact.

---

## 7. Порядок внедрения: вертикальные срезы, не одна мегазадача

Детерминированный foundation предшествует LLM orchestration. Исправления уже воспроизводимой потери
Edit и payload limits не надо откладывать до полного goal framework; их можно сделать независимыми
маленькими изменениями с regression tests. Ниже — зависимости long-running режима, не право менять
код без отдельной задачи пользователя.

| Шаг | Срез | Готовность / зависимости |
|---|---|---|
| 0 | Fixtures из AUDIT, fake clock/provider, replay/fault injection; baseline текущего поведения | Без реальных подписочных запросов и внешнего oracle; воспроизводим гонку/state/quota/overflow/crash |
| 1 | Approved goal contract, authoritative events/reducer, runtime ownership, basic states, checkpoint | Revisions/idempotency/approval и guards работают без LLM; existing sessionstore расширяется versioned |
| 2a | Watchdog, deadlines/active budgets, deterministic reports и bounded output/error retention | Можно остановить ночной run и выдать facts без Judge; fallback не ослабляет контракт |
| 2b | Quota snapshots + normalized rate-limit + durable pause/refresh/restart | Account controller и unknown policy; не нужен rollover или model router для исправления limit death |
| 3 | Candidate/manifest-bound verify, acceptance и final integration gate | Доработка существующего Verifier; cancel/timeout не pass; stale pass не закрывает новый snapshot |
| 4 | Три проекции, context injection/recitation и bounded pre-compaction checkpoint | Один writer state/journal; память живёт вне summary; проверены provider/cache/payload limits |
| 5 | Quiescent rollover, chain workspace, leases/fencing/lineage | Crash replay на каждой фазе, не два Writer-а; ожидания и budgets не сбрасываются |
| 6 | Reviewer в независимой copy; bounded advisory Judge и role subbudgets | Измерены overhead/качество; review связан с candidate; Judge не заменяет hard guards |
| 7 | Read windows/Outline/ledger, затем LSP; profile schema/manifest/HarnessStatus | Navigation quality и actual capabilities измеримы; без самоподмены executable plugins |
| 8 | Минимальный level B: findings/proposal/approval, один worktree Writer, replay+Reviewer | Использует уже существующие evidence/lease/approval; защищённые surfaces и human deployment |
| 9 | Design/reflect, сравнимый archive, curated memory и staged plugin bundles | Limits/privacy/provenance; improvements оцениваются против approved default baseline |
| 10 | Researchers/Experimenters DAG; перенос running ops/parallel writers — отдельные поздние opt-in | Independent isolation и manager integration, дополнительные recovery tests |

Legacy interactive sessions сохраняют своё поведение. Long-running mode включается явно и не импортирует
старый `status: done` как authoritative acceptance. Migration сохраняет историю; неподдерживаемая версия —
понятный отказ. У MVP можно временно не включать optional roles, но MUST-инварианты активированных
механизмов не «отключаются ради запуска»; недоступные guarantees видны в contract/status.

---

## 8. Проверки и критерии приёмки самого дизайна

### 8.1. Детерминированные safety/recovery tests

| Сценарий | Обязательный результат |
|---|---|
| Полный Write проекции state, подмена verify/history/contract; `verify: true` или `VERIFY=off` | Authoritative facts/gates неизменны; rejected или regenerated projection, событие нарушения |
| Два GoalUpdate одной revision, concurrent journal/results | Нет silent lost update; один принят, другой conflict либо согласованный batch; replay строит тот же state |
| Повтор request/event после crash | Одна принятая мутация/child session, старый результат возвращается по idempotency key |
| Pass, затем изменение tracked/untracked/fixture/dependency | История pass сохранена, но stale для нового candidate; `done/completed` не проходит старым evidence |
| Cancel/timeout/failed executor/нет assertions | Ни один не становится pass; waiver только отдельным user approval |
| Reviewer проверяет другой snapshot или test пишет входные файлы | Acceptance отклонена/inconclusive; активный Writer workspace не изменён Reviewer |
| Source test удалён или approved oracle ослаблен | Diff виден review; изменение trusted gate отклонено без нового approval |
| 429 short/weekly/transient/unknown; один cooldown аккаунт при другом eligible | Правильная классификация и route/wait; никакого `maxFailures` termination для подтверждённого quota wait |
| Quota API stale/error, общий + scoped limit, reset/rotation/sliding decrease | Unknown не ноль; constraints совместны; дельты несравнимых epoch не приписываются роли |
| Restart quota waiting, два timer fires, cancel/deadline в момент resume | Один resume dispatch либо terminal stop; counters/deadline сохранены |
| Crash до/после handoff persist, child create, old owner stop, lease transfer | Child уникален; recovery безопасен; одновременно максимум один активный Writer |
| Unknown outcome mutating shell/HTTP или старый процесс ещё жив | Нет слепого повтора/второго Writer; reconcile/ask с описанием uncertainty |
| Agent-generated plugin изменён после approval, symlink/override/core/UI попытка | Approved bundle не меняется; новые capabilities не активируются; backend отвергает fake approval |
| Level B scope расширился до protected surface; основная chain пишет тот же repo | Дополнительное разрешение до правки; сериализация lease; никакого auto merge/deploy |
| Research без Edit, повторные чтения без novelty, пустые edits, Judge timeout | Полезные artifacts учитываются; пустая активность не маскирует stuck; guards/report работают без Judge |
| Error output с мегабайтами, много изображений, disk full | Bounded context/payload; artifact refs сохранены; явный отказ до новых side effects |

Для `trusted_local` не заявляем, что тест allowlist доказывает OS isolation; strict isolation проверяется
отдельными escape tests backend-а. Fixtures не должны содержать tokens/raw private logs. Replay 234b
воспроизводит **последовательность событий и решения контроллера**, не ответы модели или успешность
исходной внешней задачи. Детерминированные тесты используют fake clock/LLM/quota; live прогоны — opt-in.

### 8.2. Эффективность: baseline и абляции

До «самоулучшения» сравниваем approved single-Writer baseline с последовательными additions:
память → watchdog/quota → rollover → Reviewer → Judge/helpers → design/reflect. Набор репозиториев
и одинаковые goals/gates; случайность отмечаем, повторения — по budget. Model/provider versions и
изменения quota plan не смешиваем в одной «средней экономии».

Измерения: accepted goal rate и качество; active time до первого verified progress; repeated unchanged
reads; время без progress; число ненужных rollover/false stuck stops; review findings и доля подтверждённых;
requests/latency/compaction counts; account quota observations с attribution confidence; доля judge/reflect
в расходе; успешное recovery после faults. Coverage/коммиты/число тулов — вспомогательные показатели,
не самостоятельное доказательство полезности.

Для replay 234b: контроллер обнаруживает воспроизведённую гонку/повторы, предлагает workaround/escalation,
выдаёт отчёты и прекращает бессодержательное продолжение в пределах configured watchdog, а не через
7 часов. Для quota нет исходных utilization данных: проверяем synthetic limit scenarios, не сочиняем
«сэкономленные weekly пункты». Конкретные efficiency thresholds утверждаем после baseline; safety
инварианты обязательны до первого автономного запуска.

---

## 9. Открытые решения перед реализацией

1. Конкретный runtime backend/расположение и schema evolution: reuse sessionstore vs chain journal;
   один owner и recoverable projections обязательны в обоих вариантах.
2. Snapshot backend для dirty/non-Git workspaces и минимальный dependency/environment fingerprint
   для каждого verify kind; policy исключения generated files подтверждается, а не угадывается.
3. Gateway interface для actual account routing/binding windows/RetryAt; какие данные confirmed,
   какие observed и какие недоступны. Никакой точной per-role quota цены без достаточных данных.
4. Hardening текущего sandbox: отдельные role copies, единый execution boundary всех tools/plugins,
   недоступность runtime/approved artifacts и shared container/mount ограничения.
5. Review/skip/human-acceptance policy для research и audit, где качество не определяется exit code;
   default sample size и thresholds сначала измеряются на локальных задачах.
6. Совместимость binary/store при level B rollout и координация с cockpit rebuild. Только безопасный
   quiescent handoff; live supervisor перенос операций и distributed leases — вне MVP.

Эти вопросы не повод снова читать весь репозиторий неделями: у каждого implementation slice есть
локальный timebox, тестируемый контракт и честный fallback/отказ. Цель v3 — сделать длинный запуск
**наблюдаемым, ограниченным и восстанавливаемым**, а не гарантировать, что модель решит любую задачу.

---

## Источники и границы применимости

Внешние ссылки перенесены из v2 (там указана проверка 2026-10-01); **в этой редакции внешняя проверка
не повторялась**. Они объясняют выбранные паттерны, но не определяют наши runtime contracts и не
заменяют локальный benchmark. Версии Claude Code, числа bugs/PR, throughput, лимиты и arXiv claims
нельзя использовать как неизменяемые provider guarantees. Для точных API/квот при реализации нужен
актуальный primary source или явное обозначение unknown.

### Локальные основания

- [Аудит 234b16c9](AUDIT.md), [дизайн v1](DESIGN-long-running-v1.md).
- [Session store](../harness/sessionstore/sessionstore.go), [coordinator dependencies](../harness/coordinator/coordinator.go)
  и [loop](../harness/coordinator/loop.go): восстановление, существующие операции, verify и retry limits.
- [Verifier](../cmd/internal/agentrunner/verify.go), [sandbox](../cmd/internal/agentrunner/sandbox.go),
  [quota](../cmd/internal/accounts/quota.go), [plugins](../docs/plugins.md), [Makefile](../Makefile).

### Внешние ориентиры

- Cognition: [Don't Build Multi-Agents](https://cognition.com/blog/dont-build-multi-agents),
  [Multi-Agents: What's Actually Working](https://cognition.com/blog/multi-agents-working) — single-writer,
  чистое review и capability routing; опубликованные показатели не обещают такой же эффект здесь.
- Claude Code: [goal](https://code.claude.com/docs/en/goal), [hooks](https://code.claude.com/docs/en/hooks),
  [wait for a usage limit](https://code.claude.com/docs/en/interactive-mode#wait-for-a-usage-limit-to-reset) —
  примеры judge/check-in/resume; версия и weekly policy требуют проверки перед переносом.
- Anthropic: [effective harnesses](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents),
  [long-running apps](https://www.anthropic.com/engineering/harness-design-long-running-apps),
  [cwc-long-running-agents](https://github.com/anthropics/cwc-long-running-agents),
  [Managed Agents](https://claude.com/blog/claude-managed-agents).
- [Manus context engineering](https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus),
  [Letta MemFS](https://www.letta.com/blog/our-next-phase), [sleep-time](https://www.letta.com/blog/sleep-time-compute)
  — файловая память, recitation и отрицательный опыт.
- [ADAS](https://github.com/ShengranHu/ADAS), [DGM](https://sakana.ai/dgm) — design/run/archive и риск objective
  hacking. [Hermes обзор](https://fastino.ai/blog/hermes-agent-the-complete-guide-to-the-self-improving-ai-agent-(2026)),
  [dreaming обзор](https://kenhuangus.substack.com/p/why-ai-agents-are-starting-to-dream) — вторичные источники;
  не основание расширять права модели.
- [Serena](https://lobehub.com/mcp/raheem-19-serena-optimized), [CocoIndex](https://cocoindex.io/blogs/index-codebase-v1),
  [Turbopuffer/ContextBench](https://www.youtube.com/watch?v=zKk7sDMGDEQ) — навигация/поиск; проверяем локально.
- [Model routing](https://arxiv.org/html/2609.28919v1), [Augment guide](https://www.augmentcode.com/guides/ai-model-routing-guide),
  [Codex context management](https://community.openai.com/t/experimental-context-management-compaction-in-codex/1395578),
  [Codex #27130](https://github.com/openai/codex/issues/27130), [Ralph](https://github.com/snarktank/ralph).
- Оценки лимитов сообщества из v2: [Morph](https://www.morphllm.com/claude-code-usage-limits),
  [ExplainX](https://explainx.ai/blog/claude-usage-limits-2026-timeline-explained),
  [Portkey](https://portkey.ai/blog/claude-code-limits),
  [Heyuan](https://www.heyuan110.com/posts/ai/2026-02-28-claude-rate-limits).
  Не используем их таблицы для hard routing/reserve; фактический endpoint — в локальном quota adapter.
