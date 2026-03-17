// Package streambench probes HTTP(S) stream URLs to estimate throughput.
//
// A single probe fetches the first 64 KB with a Range request and measures:
//   - Time to first byte (latency)
//   - Bytes/second over the entire fetch (speed)
//
// Non-HTTP URLs (magnet:, .torrent) return an ErrNotHTTP error so callers
// can fall back to seeder-count estimation.
package streambench

import (
	"errors"
	"io"
	"net/http"
	"strings"
	"time"
)

// ErrNotHTTP is returned when the URL cannot be probed with an HTTP range
// request (e.g. magnet links, local files, RTMP).
var ErrNotHTTP = errors.New("not an HTTP stream")

// ProbeSize is the number of bytes fetched per probe.
const ProbeSize = 64 * 1024 // 64 KB

// ProbeTimeout is the maximum time allowed for a single probe.
const ProbeTimeout = 8 * time.Second

// Result holds the outcome of one probe.
type Result struct {
	URL       string
	SpeedMbps float64 // megabits per second; 0 if unavailable
	LatencyMs int     // time to first byte in milliseconds
	Err       error
}

var probeClient = &http.Client{Timeout: ProbeTimeout}

// Probe fetches the first ProbeSize bytes of url and measures throughput.
// It returns ErrNotHTTP for non-HTTP(S) URLs without making any request.
func Probe(url string) Result {
	lower := strings.ToLower(url)
	if !strings.HasPrefix(lower, "http://") && !strings.HasPrefix(lower, "https://") {
		return Result{URL: url, Err: ErrNotHTTP}
	}

	req, err := http.NewRequest("GET", url, nil)
	if err != nil {
		return Result{URL: url, Err: err}
	}
	req.Header.Set("Range", "bytes=0-65535")
	req.Header.Set("User-Agent", "stui/1.0 stream-probe")

	t0 := time.Now()
	resp, err := probeClient.Do(req)
	if err != nil {
		return Result{URL: url, Err: err}
	}
	defer resp.Body.Close()

	latency := int(time.Since(t0).Milliseconds())

	// Drain up to ProbeSize bytes.
	n, err := io.Copy(io.Discard, io.LimitReader(resp.Body, ProbeSize))
	elapsed := time.Since(t0)
	if err != nil && n == 0 {
		return Result{URL: url, Err: err}
	}

	if elapsed <= 0 || n == 0 {
		return Result{URL: url, LatencyMs: latency}
	}

	speedBps := float64(n) / elapsed.Seconds()
	speedMbps := speedBps * 8 / 1_000_000

	return Result{
		URL:       url,
		SpeedMbps: speedMbps,
		LatencyMs: latency,
	}
}
