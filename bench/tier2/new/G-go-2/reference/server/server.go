// Package server exposes the engine over HTTP.
package server

import (
	"encoding/json"
	"io"
	"net/http"
	"strconv"
	"strings"

	"example.com/throttle/engine"
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

func (h *handler) check(w http.ResponseWriter, r *http.Request) {
	body, err := io.ReadAll(r.Body)
	var req checkRequest
	if err == nil {
		err = json.Unmarshal(body, &req)
	}
	if err != nil || req.Method == "" || req.Path == "" {
		writeError(w, http.StatusBadRequest, "bad request")
		return
	}
	d := h.e.Check(engine.Request{Method: req.Method, Path: req.Path, User: req.User, IP: req.IP})
	resp := checkResponse{d.Allowed, d.Matched, d.Denied, d.RetryAfter, d.Remaining}
	if resp.Matched == nil {
		resp.Matched = []string{}
	}
	if d.Allowed {
		writeJSON(w, http.StatusOK, resp)
		return
	}
	w.Header().Set("Retry-After", strconv.Itoa(d.RetryAfter))
	writeJSON(w, http.StatusTooManyRequests, resp)
}

func (h *handler) policies(w http.ResponseWriter) {
	out := []policyView{}
	for _, p := range h.e.Policies() {
		methods := "*"
		if p.Methods != nil {
			methods = strings.Join(p.Methods, ",")
		}
		out = append(out, policyView{p.Name, methods, p.Pattern, p.Limit, int64(p.Window.Seconds()), p.Burst, p.By})
	}
	writeJSON(w, http.StatusOK, out)
}
