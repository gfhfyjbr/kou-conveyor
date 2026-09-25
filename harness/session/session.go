// Package session defines durable session and turn identity.
package session

import "time"

type ID string

type TurnID string

type TurnType string

const (
	TurnRegular    TurnType = "regular"
	TurnCompaction TurnType = "compaction"
)

type Session struct {
	ID        ID
	CreatedAt time.Time
}

type Turn struct {
	ID             TurnID
	PreviousTurnID TurnID
	Type           TurnType

	// Model is the model the turn's request went to. A session may change
	// models from one prompt to the next, and this records which answered;
	// sessions recorded before it was kept leave it empty.
	Model string `json:",omitzero"`
}
