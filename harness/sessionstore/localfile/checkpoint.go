package localfile

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"fmt"
	"maps"
	"slices"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
)

const checkpointVersion = 1

var checkpointPrefix = []byte("\n{\"type\":\"checkpoint\",\"data\":")

type checkpointRecord struct {
	Version int
	Offset  int64
	SHA256  string
	Data    jsontext.Value
}

type checkpointData struct {
	Head  checkpointHead
	State jsontext.Value
}

type checkpointHead struct {
	Snapshot         sessionstore.Snapshot
	Sequence         sessionstore.Sequence
	LatestTurn       session.TurnID
	Turns            []session.TurnID
	OwnedTurns       []session.TurnID
	RespondedTurns   []session.TurnID
	Calls            []checkpointCall
	Operations       []checkpointOperation
	ExternalInputIDs []inbox.ID
}

type checkpointCall struct {
	Turn session.TurnID
	Call string
}

type checkpointOperation struct {
	Operation operation.Operation
	// Offset points to the original full state, not to a later status that
	// merely repeats it. One-based, so zero cannot name the session header.
	Offset   int64
	Archived bool
	Pending  bool
}

// SaveCheckpoint appends an acceleration record at the current item boundary.
// The original records remain authoritative for archival reads and forks.
func (store *Store) SaveCheckpoint(ctx context.Context, id session.ID, state jsontext.Value) error {
	if len(state) == 0 || !state.IsValid() {
		return fmt.Errorf("checkpoint state is not valid JSON")
	}
	head, committedSize, err := store.loadWriteState(ctx, id)
	if err != nil {
		return err
	}
	if head.itemSequence == 0 {
		return fmt.Errorf("cannot checkpoint an empty session")
	}
	if head.operationOffsets == nil {
		if err := store.locateOperations(id, &head, committedSize); err != nil {
			return err
		}
	}
	data, err := json.Marshal(checkpointData{Head: head.checkpointHead(), State: state})
	if err != nil {
		return fmt.Errorf("encode checkpoint state: %w", err)
	}
	digest := sha256.Sum256(data)
	return store.append(id, head, committedSize, recordCheckpoint, checkpointRecord{
		Version: checkpointVersion, Offset: committedSize,
		SHA256: hex.EncodeToString(digest[:]), Data: data,
	})
}

func (head sessionHead) checkpointHead() checkpointHead {
	value := checkpointHead{
		Snapshot: head.Snapshot, Sequence: head.itemSequence, LatestTurn: head.latestTurnID,
		Turns:            slices.Collect(maps.Keys(head.turns)),
		OwnedTurns:       slices.Collect(maps.Keys(head.ownedTurns)),
		RespondedTurns:   slices.Collect(maps.Keys(head.respondedTurns)),
		ExternalInputIDs: head.externalInputIDs,
	}
	for key := range head.toolCallStatuses {
		value.Calls = append(value.Calls, checkpointCall{Turn: key.turnID, Call: key.callID})
	}
	for index, operation := range head.Operations {
		_, pending := head.pendingOperations[operation.ID]
		archived := terminalOperationStatus(operation.Status) && !pending
		if archived {
			operation.State, operation.Idempotency = nil, nil
		}
		value.Operations = append(value.Operations, checkpointOperation{
			Operation: operation, Offset: head.operationOffsets[index], Archived: archived, Pending: pending,
		})
	}
	return value
}

func (value checkpointHead) restore(header session.Session, offset int64) (sessionHead, error) {
	if value.Snapshot.Session != header || value.Sequence == 0 {
		return sessionHead{}, fmt.Errorf("checkpoint session or sequence differs from its log")
	}
	head := newSessionHead(header.ID, header.CreatedAt)
	head.itemSequence, head.latestTurnID = value.Sequence, value.LatestTurn
	head.externalInputIDs = value.ExternalInputIDs
	head.operationOffsets = make([]int64, 0, len(value.Operations))
	head.archivedOperations = make(map[operation.ID]bool)
	for _, id := range value.Turns {
		if id == "" {
			return sessionHead{}, fmt.Errorf("empty checkpoint turn")
		}
		head.turns[id] = struct{}{}
	}
	if head.latestTurnID != "" {
		if _, exists := head.turns[head.latestTurnID]; !exists {
			return sessionHead{}, fmt.Errorf("checkpoint latest turn is missing")
		}
	}
	for _, id := range value.OwnedTurns {
		if _, exists := head.turns[id]; !exists {
			return sessionHead{}, fmt.Errorf("checkpoint owned turn is missing")
		}
		head.ownedTurns[id] = struct{}{}
	}
	for _, id := range value.RespondedTurns {
		if _, exists := head.ownedTurns[id]; !exists {
			return sessionHead{}, fmt.Errorf("checkpoint response turn is missing")
		}
		head.respondedTurns[id] = struct{}{}
	}
	for _, call := range value.Calls {
		if _, exists := head.ownedTurns[call.Turn]; !exists || call.Call == "" {
			return sessionHead{}, fmt.Errorf("checkpoint call has no owned turn or ID")
		}
		head.toolCallStatuses[toolCallStatusKey{turnID: call.Turn, callID: call.Call}] = struct{}{}
	}
	for _, entry := range value.Operations {
		op := entry.Operation
		if err := validateOperation(op); err != nil {
			return sessionHead{}, err
		}
		if _, exists := head.operationPositions[op.ID]; exists || entry.Offset <= 0 || entry.Offset > offset {
			return sessionHead{}, fmt.Errorf("invalid checkpoint operation location or duplicate ID")
		}
		if entry.Archived && (!terminalOperationStatus(op.Status) || entry.Pending || len(op.State) != 0 || len(op.Idempotency) != 0) {
			return sessionHead{}, fmt.Errorf("invalid archived checkpoint operation")
		}
		head.operationPositions[op.ID] = len(head.Operations)
		head.Operations = append(head.Operations, op)
		head.operationOffsets = append(head.operationOffsets, entry.Offset)
		if entry.Archived {
			head.archivedOperations[op.ID] = true
		}
		if entry.Pending {
			head.pendingOperations[op.ID] = struct{}{}
		}
	}
	return head, nil
}

// The first checkpoint indexes legacy history once. Later heads carry these
// locations forward as records are appended, without rereading old outputs.
func (store *Store) locateOperations(id session.ID, head *sessionHead, committedSize int64) error {
	head.operationOffsets = make([]int64, len(head.Operations))
	if len(head.Operations) == 0 {
		return nil
	}
	encoded, mapped, err := mapFile(store.sessionPath(id))
	if err != nil {
		return err
	}
	defer mapped.release()
	if int64(len(encoded)) < committedSize {
		return fmt.Errorf("session shrank while indexing operations")
	}
	var offset int64
	for line := range bytes.Lines(encoded[:committedSize]) {
		var record logRecord
		if err := json.Unmarshal(bytes.TrimSuffix(line, []byte{'\n'}), &record, aliasing(encoded)); err != nil {
			return err
		}
		switch record.Type {
		case recordOperation:
			var value struct{ Operation struct{ ID operation.ID } }
			if err := json.Unmarshal(record.Data, &value); err != nil {
				return err
			}
			if index, exists := head.operationPositions[value.Operation.ID]; exists {
				head.operationOffsets[index] = offset + 1
			}
		case recordItem:
			var value struct {
				Item       struct{ Kind sessionstore.ItemKind }
				Operations []jsontext.Value
			}
			if err := json.Unmarshal(record.Data, &value, aliasing(encoded)); err != nil {
				return err
			}
			if value.Item.Kind == sessionstore.ItemFork {
				clear(head.operationOffsets)
			}
			for _, raw := range value.Operations {
				if raw.Kind() == '"' {
					continue
				}
				var value struct{ ID operation.ID }
				if err := json.Unmarshal(raw, &value); err != nil {
					return err
				}
				if index, exists := head.operationPositions[value.ID]; exists && head.operationOffsets[index] == 0 {
					head.operationOffsets[index] = offset + 1
				}
			}
		}
		offset += int64(len(line))
	}
	for _, offset := range head.operationOffsets {
		if offset == 0 {
			return fmt.Errorf("operation has no original log record")
		}
	}
	return nil
}

func (head *sessionHead) noteRecord(kind recordType, value any, offset int64) {
	if head.operationOffsets == nil {
		return
	}
	for len(head.operationOffsets) < len(head.Operations) {
		head.operationOffsets = append(head.operationOffsets, 0)
	}
	switch kind {
	case recordOperation:
		value := value.(operationRecord).Operation
		head.operationOffsets[head.operationPositions[value.ID]] = offset + 1
		delete(head.archivedOperations, value.ID)
	case recordItem:
		for _, value := range value.(itemRecord).Operations {
			if value.Saved {
				continue
			}
			index, exists := head.operationPositions[value.ID]
			if exists && head.operationOffsets[index] == 0 {
				head.operationOffsets[index] = offset + 1
			}
		}
	}
}

func decodeResumeLog(encoded []byte, options ...json.Options) (storedState, int64, error) {
	committedSize := bytes.LastIndexByte(encoded, '\n') + 1
	if committedSize == 0 {
		return decodeLog(encoded, options...)
	}
	firstEnd := bytes.IndexByte(encoded[:committedSize], '\n') + 1
	headerState, _, err := decodeLog(encoded[:firstEnd], options...)
	if err != nil {
		return storedState{}, 0, err
	}
	search := encoded[:committedSize]
	for {
		index := bytes.LastIndex(search, checkpointPrefix)
		if index < 0 {
			return decodeLog(encoded, options...)
		}
		offset := index + 1
		end := offset + bytes.IndexByte(encoded[offset:committedSize], '\n') + 1
		var record logRecord
		var checkpoint checkpointRecord
		var data checkpointData
		valid := json.Unmarshal(encoded[offset:end-1], &record, options...) == nil &&
			json.Unmarshal(record.Data, &checkpoint, options...) == nil &&
			checkpoint.Version == checkpointVersion && checkpoint.Offset == int64(offset)
		if valid {
			digest := sha256.Sum256(checkpoint.Data)
			valid = checkpoint.SHA256 == hex.EncodeToString(digest[:]) &&
				json.Unmarshal(checkpoint.Data, &data, options...) == nil && len(data.State) != 0 && data.State.IsValid()
		}
		if valid {
			head, err := data.Head.restore(headerState.Snapshot.Session, int64(offset))
			if err == nil {
				state := storedState{sessionHead: head, checkpoint: &sessionstore.ResumeCheckpoint{State: data.State, After: head.itemSequence}}
				return replayCheckpointSuffix(encoded, state, end, committedSize, options...)
			}
		}
		search = search[:index]
	}
}

func replayCheckpointSuffix(encoded []byte, state storedState, offset, committedSize int, options ...json.Options) (storedState, int64, error) {
	for offset < committedSize {
		end := offset + bytes.IndexByte(encoded[offset:committedSize], '\n') + 1
		var record logRecord
		if err := json.Unmarshal(encoded[offset:end-1], &record, options...); err != nil {
			return storedState{}, 0, fmt.Errorf("decode record at byte %d: %w", offset, err)
		}
		switch record.Type {
		case recordItem:
			var value itemRecord
			if err := json.Unmarshal(record.Data, &value, options...); err != nil {
				return storedState{}, 0, fmt.Errorf("decode item record: %w", err)
			}
			if value.Item.Kind == sessionstore.ItemFork {
				return decodeLog(encoded, options...)
			}
			if value.Item.Sequence != state.itemSequence+1 || value.Item.RecordedAt.IsZero() ||
				(value.Item.Kind != sessionstore.ItemToolCallStatus && len(value.Operations) != 0) {
				return storedState{}, 0, fmt.Errorf("invalid item after checkpoint at byte %d", offset)
			}
			for _, operation := range value.Operations {
				if operation.Saved && state.archivedOperations[operation.ID] {
					if err := state.loadArchivedOperation(encoded, operation.ID, options...); err != nil {
						return storedState{}, 0, err
					}
				}
			}
			if err := replayItem(&state, value); err != nil {
				return storedState{}, 0, fmt.Errorf("apply item after checkpoint: %w", err)
			}
			state.noteRecord(recordItem, value, int64(offset))
		case recordOperation:
			var value operationRecord
			if err := json.Unmarshal(record.Data, &value, options...); err != nil {
				return storedState{}, 0, fmt.Errorf("decode operation record: %w", err)
			}
			if err := state.saveOperation(value.Operation); err != nil {
				return storedState{}, 0, err
			}
			state.noteRecord(recordOperation, value, int64(offset))
		case recordCheckpoint:
			// Invalid or newer checkpoint versions are acceleration hints only.
		case recordSession:
			return storedState{}, 0, fmt.Errorf("session record must be first")
		default:
			return storedState{}, 0, fmt.Errorf("unsupported record type %q", record.Type)
		}
		offset = end
	}
	return state, int64(committedSize), nil
}

func (head *sessionHead) loadArchivedOperation(encoded []byte, id operation.ID, options ...json.Options) error {
	index := head.operationPositions[id]
	offset := head.operationOffsets[index] - 1
	if offset < 0 || offset >= int64(len(encoded)) || (offset > 0 && encoded[offset-1] != '\n') {
		return fmt.Errorf("invalid archived operation %q location", id)
	}
	end := bytes.IndexByte(encoded[offset:], '\n')
	if end < 0 {
		return fmt.Errorf("uncommitted archived operation %q", id)
	}
	var record logRecord
	if err := json.Unmarshal(encoded[offset:offset+int64(end)], &record, options...); err != nil {
		return err
	}
	var value operation.Operation
	switch record.Type {
	case recordOperation:
		var update operationRecord
		if err := json.Unmarshal(record.Data, &update, options...); err != nil {
			return err
		}
		value = update.Operation
	case recordItem:
		var item itemRecord
		if err := json.Unmarshal(record.Data, &item, options...); err != nil {
			return err
		}
		for _, operation := range item.Operations {
			if !operation.Saved && operation.ID == id {
				value = operation.Operation
				break
			}
		}
	}
	prior := head.Operations[index]
	if value.ID != id || value.Type != prior.Type || value.Version != prior.Version || value.Status != prior.Status {
		return fmt.Errorf("archived operation %q differs from its checkpoint", id)
	}
	head.Operations[index] = value
	delete(head.archivedOperations, id)
	return nil
}
