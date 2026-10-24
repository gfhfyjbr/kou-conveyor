package llm

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"fmt"
)

type itemJSON Item

func (item Item) Validate() error {
	switch item.Type {
	case ItemMessage:
		if _, ok := item.Data.(Message); !ok {
			return fmt.Errorf("message data must be llm.Message, got %T", item.Data)
		}
	case ItemToolCall:
		if _, ok := item.Data.(ToolCall); !ok {
			return fmt.Errorf("tool call data must be llm.ToolCall, got %T", item.Data)
		}
	case ItemToolResult:
		if _, ok := item.Data.(ToolResult); !ok {
			return fmt.Errorf("tool result data must be llm.ToolResult, got %T", item.Data)
		}
	case ItemReasoning:
		if _, ok := item.Data.(Reasoning); !ok {
			return fmt.Errorf("reasoning data must be llm.Reasoning, got %T", item.Data)
		}
	default:
		return fmt.Errorf("unsupported item type %q", item.Type)
	}
	return nil
}

func (item Item) MarshalJSON() ([]byte, error) {
	if err := item.Validate(); err != nil {
		return nil, err
	}
	return json.Marshal(itemJSON(item))
}

func (item *Item) UnmarshalJSON(encoded []byte) error {
	return item.unmarshal(encoded)
}

// UnmarshalJSONFrom decodes an item as UnmarshalJSON does, but with the
// unmarshalers the decoder was given (json.WithUnmarshalers), for what the
// item holds as well: a session store that maps its files into memory
// decodes raw JSON values, as reasoning's Raw, with them as the parts of the
// mapping they are.
func (item *Item) UnmarshalJSONFrom(decoder *jsontext.Decoder) error {
	encoded, err := decoder.ReadValue()
	if err != nil {
		return err
	}
	return item.unmarshal(encoded, unmarshalers(decoder)...)
}

func (item *Item) unmarshal(encoded []byte, options ...json.Options) error {
	var decoded struct {
		itemJSON
		Data jsontext.Value
	}
	if err := json.Unmarshal(encoded, &decoded, options...); err != nil {
		return err
	}

	data, err := decodeItemData(decoded.Type, decoded.Data, options...)
	if err != nil {
		return err
	}
	decoded.itemJSON.Data = data
	*item = Item(decoded.itemJSON)
	return nil
}

// unmarshalers is the option of the unmarshalers a decoder was given, if it
// was given any.
func unmarshalers(decoder *jsontext.Decoder) []json.Options {
	value, ok := json.GetOption(decoder.Options(), json.WithUnmarshalers)
	if !ok || value == nil {
		return nil
	}
	return []json.Options{json.WithUnmarshalers(value)}
}

func decodeItemData(kind ItemType, encoded jsontext.Value, options ...json.Options) (any, error) {
	if encoded.Kind() == jsontext.KindNull {
		return nil, fmt.Errorf("%s data must not be null", kind)
	}
	switch kind {
	case ItemMessage:
		var value Message
		if err := json.Unmarshal(encoded, &value, options...); err != nil {
			return nil, fmt.Errorf("decode message data: %w", err)
		}
		return value, nil
	case ItemToolCall:
		var value ToolCall
		if err := json.Unmarshal(encoded, &value, options...); err != nil {
			return nil, fmt.Errorf("decode tool call data: %w", err)
		}
		return value, nil
	case ItemToolResult:
		var value ToolResult
		if err := json.Unmarshal(encoded, &value, options...); err != nil {
			return nil, fmt.Errorf("decode tool result data: %w", err)
		}
		return value, nil
	case ItemReasoning:
		var value Reasoning
		if err := json.Unmarshal(encoded, &value, options...); err != nil {
			return nil, fmt.Errorf("decode reasoning data: %w", err)
		}
		return value, nil
	default:
		return nil, fmt.Errorf("unsupported item type %q", kind)
	}
}
