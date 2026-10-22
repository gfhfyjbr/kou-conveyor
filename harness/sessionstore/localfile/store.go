// Package localfile persists session history and operation state in local files.
package localfile

import (
	"context"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
)

const sessionFileSuffix = ".session.jsonl"

type cachedWriteState struct {
	head          sessionHead
	committedSize int64
}

// cachedReadState is a session file as last decoded, with what the file was
// when it was read: which file, its size, and its modification time.
type cachedReadState struct {
	state         storedState
	committedSize int64
	file          os.FileInfo
}

type Store struct {
	directory            string
	writeStateCacheMutex sync.Mutex
	writeStateCache      map[session.ID]cachedWriteState
	readStateCacheMutex  sync.Mutex
	readStateCache       map[session.ID]cachedReadState
	observers            map[sessionstore.ObserverID]sessionstore.Observer
	observerOrder        []sessionstore.ObserverID
}

var _ sessionstore.Store = (*Store)(nil)

func New(directory string) (*Store, error) {
	if strings.TrimSpace(directory) == "" {
		return nil, fmt.Errorf("session store directory is empty")
	}
	absolute, err := filepath.Abs(directory)
	if err != nil {
		return nil, fmt.Errorf("resolve session store directory: %w", err)
	}
	if err := os.MkdirAll(absolute, 0o700); err != nil {
		return nil, fmt.Errorf("create session store directory: %w", err)
	}
	return &Store{
		directory:       absolute,
		writeStateCache: make(map[session.ID]cachedWriteState),
		readStateCache:  make(map[session.ID]cachedReadState),
		observers:       make(map[sessionstore.ObserverID]sessionstore.Observer),
	}, nil
}

func (store *Store) AddObserver(observer sessionstore.Observer) sessionstore.ObserverID {
	if observer == nil {
		return uuid.Nil()
	}
	id := uuid.New()
	store.observers[id] = observer
	store.observerOrder = append(store.observerOrder, id)
	return id
}

func (store *Store) RemoveObserver(id sessionstore.ObserverID) {
	delete(store.observers, id)
}

func (store *Store) Create(ctx context.Context, id session.ID) (sessionstore.Snapshot, error) {
	if err := validateSessionID(id); err != nil {
		return sessionstore.Snapshot{}, err
	}
	if err := context.Cause(ctx); err != nil {
		return sessionstore.Snapshot{}, err
	}

	state := newStoredState(id, time.Now().UTC())
	if err := store.publishInitialState(state); err != nil {
		return sessionstore.Snapshot{}, fmt.Errorf("create session %q: %w", id, err)
	}
	return state.Snapshot, nil
}

func (store *Store) Inspect(ctx context.Context, id session.ID) (sessionstore.Snapshot, error) {
	state, _, err := store.readState(ctx, id)
	if err != nil {
		return sessionstore.Snapshot{}, err
	}
	return state.Snapshot, nil
}

func (store *Store) Items(
	ctx context.Context,
	id session.ID,
	after sessionstore.Sequence,
	limit int,
) (sessionstore.Page, error) {
	if limit <= 0 {
		return sessionstore.Page{}, fmt.Errorf("item page limit must be positive")
	}

	state, _, err := store.sharedState(ctx, id)
	if err != nil {
		return sessionstore.Page{}, err
	}
	start := min(uint64(after), uint64(len(state.Items)))
	end := min(start+uint64(limit), uint64(len(state.Items)))
	items := append([]sessionstore.Item(nil), state.Items[start:end]...)
	nextAfter := after
	if len(items) > 0 {
		nextAfter = items[len(items)-1].Sequence
	}
	return sessionstore.Page{
		Items:     items,
		NextAfter: nextAfter,
		More:      end < uint64(len(state.Items)),
	}, nil
}

func (store *Store) AppendInput(ctx context.Context, id session.ID, input inbox.Input) error {
	head, committedSize, err := store.loadWriteState(ctx, id)
	if err != nil {
		return err
	}
	item, err := head.appendInput(input, time.Now().UTC())
	if err != nil {
		return err
	}
	if err := store.append(id, head, committedSize, recordItem, itemRecord{
		Item: item,
	}); err != nil {
		return err
	}
	store.notifyObservers(id, item)
	return nil
}

func (store *Store) AppendTurn(ctx context.Context, id session.ID, turn session.Turn) error {
	head, committedSize, err := store.loadWriteState(ctx, id)
	if err != nil {
		return err
	}
	item, err := head.appendTurn(turn, time.Now().UTC())
	if err != nil {
		return err
	}
	if err := store.append(id, head, committedSize, recordItem, itemRecord{
		Item: item,
	}); err != nil {
		return err
	}
	store.notifyObservers(id, item)
	return nil
}

func (store *Store) AppendModelResponse(
	ctx context.Context,
	id session.ID,
	response sessionstore.ModelResponse,
) error {
	head, committedSize, err := store.loadWriteState(ctx, id)
	if err != nil {
		return err
	}
	item, err := head.appendModelResponse(response, time.Now().UTC())
	if err != nil {
		return err
	}
	if err := store.append(id, head, committedSize, recordItem, itemRecord{
		Item: item,
	}); err != nil {
		return err
	}
	store.notifyObservers(id, item)
	return nil
}

func (store *Store) AppendToolCallStatus(
	ctx context.Context,
	id session.ID,
	status sessionstore.ToolCallStatus,
) error {
	head, committedSize, err := store.loadWriteState(ctx, id)
	if err != nil {
		return err
	}
	operations := status.Operations
	status.Operations = nil
	item, err := head.appendToolCallStatus(status, operations, time.Now().UTC())
	if err != nil {
		return err
	}
	if err := store.append(id, head, committedSize, recordItem, itemRecord{
		Item:       item,
		Operations: operations,
	}); err != nil {
		return err
	}
	status.Operations = operations
	item.Data = status
	store.notifyObservers(id, item)
	return nil
}

func (store *Store) SaveOperation(
	ctx context.Context,
	id session.ID,
	value operation.Operation,
) error {
	if err := validateOperation(value); err != nil {
		return err
	}
	head, committedSize, err := store.loadWriteState(ctx, id)
	if err != nil {
		return err
	}
	if err := head.saveOperation(value); err != nil {
		return err
	}
	return store.append(
		id,
		head,
		committedSize,
		recordOperation,
		operationRecord{Operation: value},
	)
}

func (store *Store) Resume(ctx context.Context, id session.ID) (sessionstore.ResumeState, error) {
	state, _, err := store.sharedState(ctx, id)
	if err != nil {
		return sessionstore.ResumeState{}, err
	}
	return state.resume(), nil
}

func (store *Store) Fork(
	ctx context.Context,
	id session.ID,
	parentID session.ID,
	previousTurnID session.TurnID,
) (sessionstore.Snapshot, error) {
	if err := validateSessionID(id); err != nil {
		return sessionstore.Snapshot{}, err
	}
	if id == parentID {
		return sessionstore.Snapshot{}, fmt.Errorf("fork session %q onto itself", id)
	}
	parent, _, err := store.sharedState(ctx, parentID)
	if err != nil {
		return sessionstore.Snapshot{}, fmt.Errorf("fork parent %q: %w", parentID, err)
	}
	state, err := forkStoredState(parent, id, previousTurnID, time.Now().UTC())
	if err != nil {
		return sessionstore.Snapshot{}, err
	}
	if err := store.publishInitialState(state); err != nil {
		return sessionstore.Snapshot{}, fmt.Errorf("fork session %q: %w", id, err)
	}
	store.notifyObservers(id, state.Items[len(state.Items)-1])
	return state.Snapshot, nil
}

func (store *Store) notifyObservers(id session.ID, item sessionstore.Item) {
	for _, observerID := range store.observerOrder {
		if observer, exists := store.observers[observerID]; exists {
			observer(id, item)
		}
	}
}

func (store *Store) readState(
	ctx context.Context,
	id session.ID,
) (storedState, int64, error) {
	if err := validateSessionID(id); err != nil {
		return storedState{}, 0, err
	}
	if err := context.Cause(ctx); err != nil {
		return storedState{}, 0, err
	}
	encoded, err := os.ReadFile(store.sessionPath(id))
	if err != nil {
		return storedState{}, 0, fmt.Errorf("read session %q: %w", id, err)
	}
	state, committedSize, err := decodeLog(encoded)
	if err != nil {
		return storedState{}, 0, fmt.Errorf("read session %q: %w", id, err)
	}
	if state.Snapshot.Session.ID != id {
		return storedState{}, 0, fmt.Errorf(
			"read session %q: file contains session %q",
			id,
			state.Snapshot.Session.ID,
		)
	}
	return state, committedSize, nil
}

// sharedState reads a session like readState, but it keeps what it decoded
// and returns that again while the file stays the same file with the same
// size and modification time. A run starts by resuming its session and paging
// its whole history in, and so decodes the file once rather than once a page:
// on a session of thousands of items, a fraction of a second rather than tens
// of seconds.
//
// The state is shared by every caller: none may change it.
func (store *Store) sharedState(
	ctx context.Context,
	id session.ID,
) (storedState, int64, error) {
	if err := validateSessionID(id); err != nil {
		return storedState{}, 0, err
	}
	if err := context.Cause(ctx); err != nil {
		return storedState{}, 0, err
	}
	// The file is examined before it is read, so one that grows while it is
	// read is remembered as it was before and read again the next time.
	file, statErr := os.Stat(store.sessionPath(id))
	if statErr == nil {
		store.readStateCacheMutex.Lock()
		cached, ok := store.readStateCache[id]
		store.readStateCacheMutex.Unlock()
		if ok && sameFileVersion(cached.file, file) {
			return cached.state, cached.committedSize, nil
		}
	}
	state, committedSize, err := store.readState(ctx, id)
	if err != nil {
		return storedState{}, 0, err
	}
	if statErr == nil {
		store.readStateCacheMutex.Lock()
		store.readStateCache[id] = cachedReadState{
			state:         state,
			committedSize: committedSize,
			file:          file,
		}
		store.readStateCacheMutex.Unlock()
	}
	return state, committedSize, nil
}

func sameFileVersion(cached, current os.FileInfo) bool {
	return os.SameFile(cached, current) &&
		cached.Size() == current.Size() &&
		cached.ModTime().Equal(current.ModTime())
}

func (store *Store) evictCachedReadState(id session.ID) {
	store.readStateCacheMutex.Lock()
	delete(store.readStateCache, id)
	store.readStateCacheMutex.Unlock()
}

func (store *Store) loadWriteState(
	ctx context.Context,
	id session.ID,
) (sessionHead, int64, error) {
	if err := validateSessionID(id); err != nil {
		return sessionHead{}, 0, err
	}
	if err := context.Cause(ctx); err != nil {
		return sessionHead{}, 0, err
	}
	if head, committedSize, ok := store.getCachedWriteState(id); ok {
		return head, committedSize, nil
	}
	state, committedSize, err := store.sharedState(ctx, id)
	if err != nil {
		return sessionHead{}, 0, err
	}
	// Writes change the head in place, and the state read is shared.
	head := state.sessionHead.clone()
	store.putCachedWriteState(id, head, committedSize)
	return head, committedSize, nil
}

func (store *Store) publishInitialState(state storedState) error {
	encoded, err := encodeInitialLog(state.Snapshot.Session, state.Items)
	if err != nil {
		return err
	}
	store.evictCachedReadState(state.Snapshot.Session.ID)
	if err := publishFile(store.directory, store.sessionPath(state.Snapshot.Session.ID), encoded); err != nil {
		store.evictCachedWriteState(state.Snapshot.Session.ID)
		return err
	}
	store.putCachedWriteState(state.Snapshot.Session.ID, state.sessionHead, int64(len(encoded)))
	return nil
}

func (store *Store) append(
	id session.ID,
	head sessionHead,
	committedSize int64,
	kind recordType,
	value any,
) error {
	encoded, err := encodeRecord(kind, value)
	if err != nil {
		store.evictCachedWriteState(id)
		return err
	}
	store.evictCachedReadState(id)
	if err := appendFile(store.sessionPath(id), committedSize, encoded); err != nil {
		store.evictCachedWriteState(id)
		return err
	}
	store.putCachedWriteState(id, head, committedSize+int64(len(encoded)))
	return nil
}

func (store *Store) getCachedWriteState(id session.ID) (sessionHead, int64, bool) {
	store.writeStateCacheMutex.Lock()
	cached, ok := store.writeStateCache[id]
	store.writeStateCacheMutex.Unlock()
	if !ok {
		return sessionHead{}, 0, false
	}
	return cached.head, cached.committedSize, true
}

func (store *Store) putCachedWriteState(id session.ID, head sessionHead, committedSize int64) {
	store.writeStateCacheMutex.Lock()
	store.writeStateCache[id] = cachedWriteState{head: head, committedSize: committedSize}
	store.writeStateCacheMutex.Unlock()
}

func (store *Store) evictCachedWriteState(id session.ID) {
	store.writeStateCacheMutex.Lock()
	delete(store.writeStateCache, id)
	store.writeStateCacheMutex.Unlock()
}

func (store *Store) sessionPath(id session.ID) string {
	return filepath.Join(store.directory, sessionFilename(id))
}

// Path returns the file that holds a session's history: JSON Lines, a
// header record and then one record per item or operation checkpoint.
func (store *Store) Path(id session.ID) string {
	return store.sessionPath(id)
}

func sessionFilename(id session.ID) string {
	return string(id) + sessionFileSuffix
}

func publishFile(directory, target string, data []byte) (err error) {
	temporary, err := os.CreateTemp(directory, ".session-*.tmp")
	if err != nil {
		return fmt.Errorf("create temporary session file: %w", err)
	}
	temporaryPath := temporary.Name()
	defer func() {
		if temporaryPath == "" {
			return
		}
		removeErr := os.Remove(temporaryPath)
		if removeErr != nil && !errors.Is(removeErr, fs.ErrNotExist) {
			err = errors.Join(err, removeErr)
		}
	}()

	if err := persistTemporary(temporary, data); err != nil {
		return err
	}

	if err := os.Rename(temporaryPath, target); err != nil {
		return fmt.Errorf("publish session file: %w", err)
	}
	temporaryPath = ""
	return syncDirectory(directory)
}

func appendFile(path string, committedSize int64, data []byte) (err error) {
	file, err := os.OpenFile(path, os.O_WRONLY|os.O_APPEND, 0)
	if err != nil {
		return fmt.Errorf("open session log for append: %w", err)
	}
	defer func() {
		if closeErr := file.Close(); closeErr != nil {
			err = errors.Join(err, fmt.Errorf("close session log: %w", closeErr))
		}
	}()
	if err := file.Truncate(committedSize); err != nil {
		return fmt.Errorf("discard incomplete session record: %w", err)
	}
	if _, err := file.Write(data); err != nil {
		return fmt.Errorf("append session record: %w", err)
	}
	if err := file.Sync(); err != nil {
		return fmt.Errorf("sync session log: %w", err)
	}
	return nil
}

func persistTemporary(file *os.File, data []byte) (err error) {
	defer func() {
		if closeErr := file.Close(); closeErr != nil {
			err = errors.Join(err, fmt.Errorf("close temporary session file: %w", closeErr))
		}
	}()
	if _, err := file.Write(data); err != nil {
		return fmt.Errorf("write temporary session file: %w", err)
	}
	if err := file.Sync(); err != nil {
		return fmt.Errorf("sync temporary session file: %w", err)
	}
	return nil
}

func syncDirectory(path string) error {
	directory, err := os.Open(path)
	if err != nil {
		return fmt.Errorf("open session store directory for sync: %w", err)
	}
	return syncDirectoryFile(directory)
}

func syncDirectoryFile(directory *os.File) (err error) {
	defer func() {
		if closeErr := directory.Close(); closeErr != nil {
			err = errors.Join(err, fmt.Errorf("close session store directory: %w", closeErr))
		}
	}()
	if err := directory.Sync(); err != nil {
		return fmt.Errorf("sync session store directory: %w", err)
	}
	return nil
}

func validateSessionID(id session.ID) error {
	name := string(id)
	if name == "" {
		return fmt.Errorf("session ID is empty")
	}
	for _, character := range name {
		if character >= 'a' && character <= 'z' ||
			character >= 'A' && character <= 'Z' ||
			character >= '0' && character <= '9' ||
			character == '-' {
			continue
		}
		return fmt.Errorf("session ID %q must contain only ASCII letters, digits, and dashes", id)
	}
	return nil
}
