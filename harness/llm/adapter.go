package llm

import "context"

type RequestOptions struct {
	CacheKey string
}

type Adapter interface {
	Respond(context.Context, Request, RequestOptions) (Response, error)
}
