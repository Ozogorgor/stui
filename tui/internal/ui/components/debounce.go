package components

import (
	"sync"
	"time"
)

type Debouncer struct {
	mu       sync.Mutex
	timer    *time.Timer
	delay    time.Duration
	lastID   int
	callback func()
}

func NewDebouncer(delay time.Duration) *Debouncer {
	return &Debouncer{
		delay: delay,
	}
}

func (d *Debouncer) Trigger(id int, callback func()) {
	d.mu.Lock()
	defer d.mu.Unlock()

	if d.timer != nil {
		d.timer.Stop()
	}
	d.lastID = id
	d.callback = callback

	d.timer = time.AfterFunc(d.delay, func() {
		d.mu.Lock()
		callback := d.callback
		d.callback = nil
		d.mu.Unlock()
		if callback != nil {
			callback()
		}
	})
}

func (d *Debouncer) Cancel() {
	d.mu.Lock()
	defer d.mu.Unlock()
	if d.timer != nil {
		d.timer.Stop()
		d.timer = nil
	}
	d.callback = nil
}

func (d *Debouncer) IsPending() bool {
	d.mu.Lock()
	defer d.mu.Unlock()
	return d.timer != nil
}

type Throttler struct {
	mu       sync.Mutex
	lastTime time.Time
	interval time.Duration
}

func NewThrottler(interval time.Duration) *Throttler {
	return &Throttler{
		interval: interval,
	}
}

func (t *Throttler) ShouldProcess() bool {
	t.mu.Lock()
	defer t.mu.Unlock()
	now := time.Now()
	if now.Sub(t.lastTime) >= t.interval {
		t.lastTime = now
		return true
	}
	return false
}

func (t *Throttler) Reset() {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.lastTime = time.Time{}
}
