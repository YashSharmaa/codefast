// Package server exposes the engine over HTTP.
package server

import (
	"bytes"
	"encoding/json"
	"io"
	"net/http"
	"strconv"
	"strings"

	"example.com/throttle/engine"
	"example.com/throttle/replay"
)

type checkRequest struct {
	Method string `json:"method"`
	Path   string `json:"path"`
	User   string `json:"user"`
	IP     string `json:"ip"`
}

type checkResponse struct {
	Allowed    bool     `json:"allowed"`
	Matched    []string `json:"matched"`
	Denied     string   `json:"denied"`
	RetryAfter int      `json:"retry_after"`
	Remaining  int      `json:"remaining"`
}

type policyView struct {
	Name          string `json:"name"`
	Methods       string `json:"methods"`
	Pattern       string `json:"pattern"`
	Limit         int    `json:"limit"`
	WindowSeconds int64  `json:"window_seconds"`
	Burst         int    `json:"burst"`
	By            string `json:"by"`
	Algo          string `json:"algo"`
	Cost          int    `json:"cost"`
}

type handler struct {
	e *engine.Engine
}

// NewHandler returns the HTTP surface of e.
func NewHandler(e *engine.Engine) http.Handler {
	return &handler{e: e}
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	json.NewEncoder(w).Encode(v)
}

func writeError(w http.ResponseWriter, status int, msg string) {
	writeJSON(w, status, map[string]string{"error": msg})
}

func (h *handler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	switch r.URL.Path {
	case "/check":
		if r.Method != http.MethodPost {
			w.Header().Set("Allow", http.MethodPost)
			writeError(w, http.StatusMethodNotAllowed, "method not allowed")
			return
		}
		h.check(w, r)
	case "/replay":
		if r.Method != http.MethodPost {
			w.Header().Set("Allow", http.MethodPost)
			writeError(w, http.StatusMethodNotAllowed, "method not allowed")
			return
		}
		h.replay(w, r)
	case "/policies":
		if r.Method != http.MethodGet {
			w.Header().Set("Allow", http.MethodGet)
			writeError(w, http.StatusMethodNotAllowed, "method not allowed")
			return
		}
		h.policies(w)
	default:
		writeError(w, http.StatusNotFound, "not found")
	}
}

// decodeRequest reads one request object, or reports that it is a bad request.
func decodeRequest(raw []byte) (engine.Request, bool) {
	var req checkRequest
	trimmed := bytes.TrimSpace(raw)
	if len(trimmed) == 0 || trimmed[0] != '{' {
		return engine.Request{}, false
	}
	if err := json.Unmarshal(raw, &req); err != nil || req.Method == "" || req.Path == "" {
		return engine.Request{}, false
	}
	return engine.Request{Method: req.Method, Path: req.Path, User: req.User, IP: req.IP}, true
}

func view(d engine.Decision) checkResponse {
	resp := checkResponse{d.Allowed, d.Matched, d.Denied, d.RetryAfter, d.Remaining}
	if resp.Matched == nil {
		resp.Matched = []string{}
	}
	return resp
}

func (h *handler) check(w http.ResponseWriter, r *http.Request) {
	body, err := io.ReadAll(r.Body)
	trimmed := bytes.TrimSpace(body)
	if err == nil && len(trimmed) > 0 && trimmed[0] == '[' {
		h.checkBatch(w, body)
		return
	}
	req, ok := decodeRequest(body)
	if err != nil || !ok {
		writeError(w, http.StatusBadRequest, "bad request")
		return
	}
	d := h.e.Check(req)
	if d.Allowed {
		writeJSON(w, http.StatusOK, view(d))
		return
	}
	w.Header().Set("Retry-After", strconv.Itoa(d.RetryAfter))
	writeJSON(w, http.StatusTooManyRequests, view(d))
}

func (h *handler) checkBatch(w http.ResponseWriter, body []byte) {
	var items []json.RawMessage
	if err := json.Unmarshal(body, &items); err != nil {
		writeError(w, http.StatusBadRequest, "bad request")
		return
	}
	var reqs []engine.Request
	var slots []int // index into reqs, or -1 for a bad element
	for _, raw := range items {
		req, ok := decodeRequest(raw)
		if !ok {
			slots = append(slots, -1)
			continue
		}
		slots = append(slots, len(reqs))
		reqs = append(reqs, req)
	}
	decisions := h.e.CheckAll(reqs)
	out := []any{}
	retry, denied := 0, false
	for _, s := range slots {
		if s < 0 {
			out = append(out, map[string]string{"error": "bad request"})
			continue
		}
		d := decisions[s]
		if !d.Allowed {
			denied = true
			retry = max(retry, d.RetryAfter)
		}
		out = append(out, view(d))
	}
	if !denied {
		writeJSON(w, http.StatusOK, out)
		return
	}
	w.Header().Set("Retry-After", strconv.Itoa(retry))
	writeJSON(w, http.StatusTooManyRequests, out)
}

type summaryView struct {
	Name    string `json:"name"`
	Matched int    `json:"matched"`
	Denied  int    `json:"denied"`
}

type resultView struct {
	Line       int    `json:"line"`
	Allowed    bool   `json:"allowed"`
	Denied     string `json:"denied"`
	RetryAfter int    `json:"retry_after"`
	Remaining  int    `json:"remaining"`
}

type replayView struct {
	Requests int           `json:"requests"`
	Allowed  int           `json:"allowed"`
	Denied   int           `json:"denied"`
	Policies []summaryView `json:"policies"`
	Results  []resultView  `json:"results"`
}

func (h *handler) replay(w http.ResponseWriter, r *http.Request) {
	body, err := io.ReadAll(r.Body)
	if err != nil {
		writeError(w, http.StatusBadRequest, "bad request")
		return
	}
	rep, err := replay.Run(h.e.Policies(), string(body))
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	out := replayView{Requests: rep.Requests, Allowed: rep.Allowed, Denied: rep.Denied, Policies: []summaryView{}, Results: []resultView{}}
	for _, s := range rep.Policies {
		out.Policies = append(out.Policies, summaryView{s.Name, s.Matched, s.Denied})
	}
	for _, res := range rep.Results {
		d := res.Decision
		out.Results = append(out.Results, resultView{res.Line, d.Allowed, d.Denied, d.RetryAfter, d.Remaining})
	}
	writeJSON(w, http.StatusOK, out)
}

func (h *handler) policies(w http.ResponseWriter) {
	out := []policyView{}
	for _, p := range h.e.Policies() {
		methods := "*"
		if p.Methods != nil {
			methods = strings.Join(p.Methods, ",")
		}
		out = append(out, policyView{p.Name, methods, p.Pattern, p.Limit, int64(p.Window.Seconds()), p.Burst, p.By, p.Algo, p.Cost})
	}
	writeJSON(w, http.StatusOK, out)
}
