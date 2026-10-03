package canvas

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/base64"
	"errors"
	"strconv"
	"strings"
)

// A node's program acts on its canvas with a token the engine gives it:
//
//	kc1.<canvas>.<node>.<epoch>.<scope>.<mac>
//
// mac signs the rest with the server's secret (LoadSecret). A token is
// void once its node is gone, or once the node's epoch moves on (a new
// token, or a restart); its scope is what it allows (allows).

const tokenPrefix = "kc1"

// claims are what a token says.
type claims struct {
	Canvas string
	Node   string
	Epoch  int
	Scope  string
}

var errToken = errors.New("invalid canvas token")

func tokenMAC(secret []byte, c claims) string {
	mac := hmac.New(sha256.New, secret)
	mac.Write([]byte(tokenPrefix + "|" + c.Canvas + "|" + c.Node + "|" + strconv.Itoa(c.Epoch) + "|" + c.Scope))
	return base64.RawURLEncoding.EncodeToString(mac.Sum(nil))
}

// mintToken makes a token of claims.
func mintToken(secret []byte, c claims) string {
	return strings.Join([]string{tokenPrefix, c.Canvas, c.Node, strconv.Itoa(c.Epoch), c.Scope, tokenMAC(secret, c)}, ".")
}

// parseToken checks a token's signature and returns what it says; whether
// its canvas and node are there, and its epoch theirs, the caller checks.
func parseToken(secret []byte, token string) (claims, error) {
	parts := strings.Split(strings.TrimSpace(token), ".")
	if len(parts) != 6 || parts[0] != tokenPrefix {
		return claims{}, errToken
	}
	epoch, err := strconv.Atoi(parts[3])
	if err != nil || epoch < 0 {
		return claims{}, errToken
	}
	c := claims{Canvas: parts[1], Node: parts[2], Epoch: epoch, Scope: parts[4]}
	if !ValidID(c.Canvas) || !nodeIDPattern.MatchString(c.Node) {
		return claims{}, errToken
	}
	if _, ok := scopeRank[c.Scope]; !ok && c.Scope != scopeSource {
		return claims{}, errToken
	}
	if !hmac.Equal([]byte(tokenMAC(secret, c)), []byte(parts[5])) {
		return claims{}, errToken
	}
	return c, nil
}
