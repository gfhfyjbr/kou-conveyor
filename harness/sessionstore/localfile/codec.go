package localfile

import (
	"bytes"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"fmt"
	"reflect"
	"unsafe"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
)

const formatVersion = 2

type recordType string

const (
	recordSession   recordType = "session"
	recordItem      recordType = "item"
	recordOperation recordType = "operation"
)

type logRecord struct {
	Type recordType     `json:"type"`
	Data jsontext.Value `json:"data"`
}

type sessionRecord struct {
	Version int
	Session session.Session
}

type itemRecord struct {
	Item       sessionstore.Item
	Operations []ItemOperation `json:",omitempty"`
}

// ItemOperation is an operation as the item record of a tool call's status
// holds it. The first status recorded for a call holds its operations whole.
// A status that repeats it, as the one recorded once they finish, holds an
// operation the file already has as it is (its latest checkpoint, or the
// first status) by its ID alone, a JSON string where the operation would be:
// the item is Saved, with only the ID set. A finished operation's state, a
// tool's whole output, is so written once rather than twice.
type ItemOperation struct {
	operation.Operation
	Saved bool
}

func (value ItemOperation) MarshalJSONTo(encoder *jsontext.Encoder) error {
	if value.Saved {
		return encoder.WriteToken(jsontext.String(string(value.ID)))
	}
	return json.MarshalEncode(encoder, value.Operation)
}

func (value *ItemOperation) UnmarshalJSONFrom(decoder *jsontext.Decoder) error {
	if decoder.PeekKind() == '"' {
		token, err := decoder.ReadToken()
		if err != nil {
			return err
		}
		*value = ItemOperation{Operation: operation.Operation{ID: operation.ID(token.String())}, Saved: true}
		return nil
	}
	*value = ItemOperation{}
	return json.UnmarshalDecode(decoder, &value.Operation)
}

// itemOperations returns the operations of a tool call's status as its item
// record holds them, from the head as it is before it takes the status in.
func (head *sessionHead) itemOperations(
	status sessionstore.ToolCallStatus,
	operations []operation.Operation,
) []ItemOperation {
	if len(operations) == 0 {
		return nil
	}
	_, repeated := head.toolCallStatuses[toolCallStatusKey{turnID: status.TurnID, callID: status.CallID}]
	values := make([]ItemOperation, len(operations))
	for index, value := range operations {
		position, saved := head.operationPositions[value.ID]
		saved = saved && repeated && reflect.DeepEqual(head.Operations[position], value)
		values[index] = ItemOperation{Operation: value, Saved: saved}
	}
	return values
}

// recordedOperations returns the operations the item record of a tool call's
// status holds, each one it holds by ID as the head has it.
func (head *sessionHead) recordedOperations(
	status sessionstore.ToolCallStatus,
	values []ItemOperation,
) ([]operation.Operation, error) {
	if len(values) == 0 {
		return nil, nil
	}
	_, repeated := head.toolCallStatuses[toolCallStatusKey{turnID: status.TurnID, callID: status.CallID}]
	operations := make([]operation.Operation, len(values))
	for index, value := range values {
		if !value.Saved {
			operations[index] = value.Operation
			continue
		}
		if !repeated {
			return nil, fmt.Errorf(
				"first status of call %q names operation %q without its state",
				status.CallID,
				value.ID,
			)
		}
		position, exists := head.operationPositions[value.ID]
		if !exists {
			return nil, fmt.Errorf(
				"status of call %q names operation %q, which the session has not saved",
				status.CallID,
				value.ID,
			)
		}
		operations[index] = head.Operations[position]
	}
	return operations, nil
}

// aliasing is the option that decodes raw JSON values, such as operations'
// states, from the bytes of a mapped session file as the parts of the
// mapping they are rather than as copies. Those of other bytes are copied.
func aliasing(mapped []byte) json.Options {
	start := uintptr(unsafe.Pointer(unsafe.SliceData(mapped)))
	end := start + uintptr(len(mapped))
	return json.WithUnmarshalers(json.UnmarshalFromFunc(
		func(decoder *jsontext.Decoder, value *jsontext.Value) error {
			raw, err := decoder.ReadValue()
			if err != nil {
				return err
			}
			at := uintptr(unsafe.Pointer(unsafe.SliceData(raw)))
			if at >= start && at+uintptr(len(raw)) <= end {
				// With no room past its end, a value appended to is copied
				// rather than written over the record that follows it.
				*value = raw[:len(raw):len(raw)]
			} else {
				*value = bytes.Clone(raw)
			}
			return nil
		},
	))
}

type operationRecord struct {
	Operation operation.Operation
}

type decodedRecord struct {
	Type      recordType
	Item      itemRecord
	Operation operation.Operation
}

func encodeInitialLog(value session.Session, items []sessionstore.Item) ([]byte, error) {
	if err := validateSessionID(value.ID); err != nil {
		return nil, err
	}

	encoded, err := encodeRecord(recordSession, sessionRecord{
		Version: formatVersion,
		Session: value,
	})
	if err != nil {
		return nil, fmt.Errorf("encode session: %w", err)
	}

	for index, item := range items {
		if item.Sequence != sessionstore.Sequence(index+1) {
			return nil, fmt.Errorf("item %d has non-contiguous sequence %d", index, item.Sequence)
		}
		if item.RecordedAt.IsZero() {
			return nil, fmt.Errorf("item %d recorded time is zero", index)
		}

		record := itemRecord{Item: item}
		if item.Kind == sessionstore.ItemToolCallStatus {
			status := item.Data.(sessionstore.ToolCallStatus)
			for _, value := range status.Operations {
				record.Operations = append(record.Operations, ItemOperation{Operation: value})
			}
			status.Operations = nil
			record.Item.Data = status
		}
		line, err := encodeRecord(recordItem, record)
		if err != nil {
			return nil, fmt.Errorf("encode item %d: %w", index, err)
		}
		encoded = append(encoded, line...)
	}
	return encoded, nil
}

// decodeLog decodes a session file's bytes with the decoding options given,
// such as aliasing.
func decodeLog(encoded []byte, options ...json.Options) (storedState, int64, error) {
	lastNewline := bytes.LastIndexByte(encoded, '\n')
	if lastNewline < 0 {
		return storedState{}, 0, fmt.Errorf("session log has no committed records")
	}
	committedSize := int64(lastNewline + 1)
	lines := bytes.Split(encoded[:committedSize], []byte{'\n'})
	lines = lines[:len(lines)-1]

	var value storedState
	records := make([]decodedRecord, 0, len(lines))
	itemCount := 0
	inheritedItemCount := 0
	for lineIndex, line := range lines {
		var record logRecord
		if err := json.Unmarshal(line, &record, options...); err != nil {
			return storedState{}, 0, fmt.Errorf("decode record %d: %w", lineIndex+1, err)
		}
		switch record.Type {
		case recordSession:
			if lineIndex != 0 {
				return storedState{}, 0, fmt.Errorf("session record must be first")
			}
			var header sessionRecord
			if err := json.Unmarshal(record.Data, &header); err != nil {
				return storedState{}, 0, fmt.Errorf("decode session record: %w", err)
			}
			if header.Version == 1 {
				return storedState{}, 0, fmt.Errorf(
					"legacy session format version 1 cannot be resumed",
				)
			}
			if header.Version != formatVersion {
				return storedState{}, 0, fmt.Errorf("unsupported session format version %d", header.Version)
			}
			if err := validateSessionID(header.Session.ID); err != nil {
				return storedState{}, 0, err
			}
			if header.Session.CreatedAt.IsZero() {
				return storedState{}, 0, fmt.Errorf("session %q creation time is zero", header.Session.ID)
			}
			value = newStoredState(header.Session.ID, header.Session.CreatedAt)
		case recordItem:
			if lineIndex == 0 {
				return storedState{}, 0, fmt.Errorf("session record must be first")
			}
			var itemValue itemRecord
			if err := json.Unmarshal(record.Data, &itemValue, options...); err != nil {
				return storedState{}, 0, fmt.Errorf("decode item record: %w", err)
			}
			wantSequence := sessionstore.Sequence(itemCount + 1)
			if itemValue.Item.Sequence != wantSequence {
				return storedState{}, 0, fmt.Errorf(
					"item %d has sequence %d, want %d",
					itemCount,
					itemValue.Item.Sequence,
					wantSequence,
				)
			}
			if itemValue.Item.RecordedAt.IsZero() {
				return storedState{}, 0, fmt.Errorf("item %d recorded time is zero", itemCount)
			}
			if itemValue.Item.Kind != sessionstore.ItemToolCallStatus && len(itemValue.Operations) != 0 {
				return storedState{}, 0, fmt.Errorf(
					"item %d of kind %q initializes operations",
					itemCount,
					itemValue.Item.Kind,
				)
			}
			itemCount++
			if itemValue.Item.Kind == sessionstore.ItemFork {
				inheritedItemCount = itemCount
			}
			records = append(records, decodedRecord{Type: recordItem, Item: itemValue})
		case recordOperation:
			if lineIndex == 0 {
				return storedState{}, 0, fmt.Errorf("session record must be first")
			}
			var update operationRecord
			if err := json.Unmarshal(record.Data, &update, options...); err != nil {
				return storedState{}, 0, fmt.Errorf("decode operation record: %w", err)
			}
			records = append(records, decodedRecord{
				Type: recordOperation, Operation: update.Operation,
			})
		default:
			return storedState{}, 0, fmt.Errorf("unsupported record type %q", record.Type)
		}
	}

	replayedItems := 0
	for recordIndex, record := range records {
		switch record.Type {
		case recordItem:
			itemIndex := replayedItems
			replayedItems++
			if itemIndex < inheritedItemCount {
				if len(record.Item.Operations) != 0 {
					return storedState{}, 0, fmt.Errorf("inherited item %d initializes operations", itemIndex)
				}
				value.inheritItem(record.Item.Item)
				continue
			}
			if err := replayItem(&value, record.Item); err != nil {
				return storedState{}, 0, fmt.Errorf("apply item %d: %w", itemIndex, err)
			}
		case recordOperation:
			if replayedItems < inheritedItemCount {
				return storedState{}, 0, fmt.Errorf(
					"operation record %d appears in inherited history",
					recordIndex+2,
				)
			}
			if err := value.saveOperation(record.Operation); err != nil {
				return storedState{}, 0, fmt.Errorf("apply operation record: %w", err)
			}
		}
	}
	return value, committedSize, nil
}

func replayItem(state *storedState, record itemRecord) error {
	switch record.Item.Kind {
	case sessionstore.ItemInput:
		return state.appendInput(record.Item.Data.(inbox.Input), record.Item.RecordedAt)
	case sessionstore.ItemTurn:
		return state.appendTurn(record.Item.Data.(session.Turn), record.Item.RecordedAt)
	case sessionstore.ItemModelResponse:
		return state.appendModelResponse(
			record.Item.Data.(sessionstore.ModelResponse),
			record.Item.RecordedAt,
		)
	case sessionstore.ItemToolCallStatus:
		status := record.Item.Data.(sessionstore.ToolCallStatus)
		operations, err := state.recordedOperations(status, record.Operations)
		if err != nil {
			return err
		}
		return state.appendToolCallStatus(status, operations, record.Item.RecordedAt)
	default:
		return fmt.Errorf("unexpected owned item kind %q", record.Item.Kind)
	}
}

func encodeRecord(kind recordType, value any) ([]byte, error) {
	data, err := json.Marshal(value)
	if err != nil {
		return nil, err
	}
	encoded, err := json.Marshal(logRecord{Type: kind, Data: data})
	if err != nil {
		return nil, err
	}
	if bytes.IndexByte(encoded, '\n') >= 0 {
		return nil, fmt.Errorf("encoded record contains a newline")
	}
	return append(encoded, '\n'), nil
}
