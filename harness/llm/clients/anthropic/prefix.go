package anthropic

import (
	"crypto/sha256"
	"encoding/base64"
	"encoding/binary"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"hash"

	sdk "github.com/anthropics/anthropic-sdk-go"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// fingerprint is a short hash of data: 48 bits, which tell two versions of
// a request part apart, and take 8 characters.
func fingerprint(data []byte) string {
	sum := sha256.Sum256(data)
	return base64.RawURLEncoding.EncodeToString(sum[:6])
}

// rolling fingerprints parts in order, each with all that came before it.
type rolling struct{ h hash.Hash }

func (r *rolling) add(part []byte) string {
	if r.h == nil {
		r.h = sha256.New()
	}
	var size [8]byte
	binary.BigEndian.PutUint64(size[:], uint64(len(part)))
	r.h.Write(size[:])
	r.h.Write(part)
	return base64.RawURLEncoding.EncodeToString(r.h.Sum(nil)[:6])
}

// defaultTTL is how long the API keeps a cache that does not say.
const defaultTTL = "5m"

// prefixOf fingerprints what params send (llm.Prefix). It takes the cache
// markers off each block while it fingerprints it and puts them back: call
// it once nothing else uses params.
func prefixOf(params sdk.BetaMessageNewParams) llm.Prefix {
	var prefix llm.Prefix
	if len(params.Tools) != 0 {
		if data, err := json.Marshal(params.Tools); err == nil {
			prefix.Tools = fingerprint(data)
		}
	}
	for index := range params.System {
		block := params.System[index]
		if block.CacheControl.Type != "" {
			prefix.Breakpoints = append(prefix.Breakpoints, breakpoint(-1, block.CacheControl))
			block.CacheControl = sdk.BetaCacheControlEphemeralParam{}
		}
		data, err := json.Marshal(block)
		if err != nil {
			return llm.Prefix{}
		}
		prefix.System = append(prefix.System, fingerprint(data))
	}
	var messages rolling
	for index, message := range params.Messages {
		var marker *sdk.BetaCacheControlEphemeralParam
		var marked sdk.BetaCacheControlEphemeralParam
		if len(message.Content) != 0 {
			if control := message.Content[len(message.Content)-1].GetCacheControl(); control != nil && control.Type != "" {
				marker, marked = control, *control
				prefix.Breakpoints = append(prefix.Breakpoints, breakpoint(index, marked))
				*control = sdk.BetaCacheControlEphemeralParam{}
			}
		}
		data, err := json.Marshal(message)
		if marker != nil {
			*marker = marked
		}
		if err != nil {
			return llm.Prefix{}
		}
		prefix.Messages = append(prefix.Messages, messages.add(data))
	}
	if len(params.ContextManagement.Edits) != 0 {
		if data, err := json.Marshal(params.ContextManagement.Edits); err == nil {
			prefix.Edits = jsontext.Value(data)
		}
	}
	return prefix
}

func breakpoint(message int, control sdk.BetaCacheControlEphemeralParam) llm.Breakpoint {
	ttl := string(control.TTL)
	if ttl == "" {
		ttl = defaultTTL
	}
	return llm.Breakpoint{Message: message, TTL: ttl}
}
