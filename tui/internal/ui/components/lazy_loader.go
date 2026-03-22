package components

import (
	"sync"
)

type LazyLoader struct {
	mu            sync.RWMutex
	loaded        map[string]bool
	loading       map[string]bool
	items         map[string]LazyItem
	onLoad        func(string)
	maxConcurrent int
	semaphore     chan struct{}
}

type LazyItem struct {
	ID       string
	Priority int
	Data     any
}

func NewLazyLoader(maxConcurrent int, onLoad func(string)) *LazyLoader {
	return &LazyLoader{
		loaded:        make(map[string]bool),
		loading:       make(map[string]bool),
		items:         make(map[string]LazyItem),
		onLoad:        onLoad,
		maxConcurrent: maxConcurrent,
		semaphore:     make(chan struct{}, maxConcurrent),
	}
}

func (l *LazyLoader) Add(id string, priority int, data any) {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.items[id] = LazyItem{ID: id, Priority: priority, Data: data}
}

func (l *LazyLoader) Remove(id string) {
	l.mu.Lock()
	defer l.mu.Unlock()
	delete(l.items, id)
	delete(l.loaded, id)
	delete(l.loading, id)
}

func (l *LazyLoader) IsLoaded(id string) bool {
	l.mu.RLock()
	defer l.mu.RUnlock()
	return l.loaded[id]
}

func (l *LazyLoader) IsLoading(id string) bool {
	l.mu.RLock()
	defer l.mu.RUnlock()
	return l.loading[id]
}

func (l *LazyLoader) MarkLoaded(id string) {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.loaded[id] = true
	delete(l.loading, id)
}

func (l *LazyLoader) Get(id string) (any, bool) {
	l.mu.RLock()
	item, ok := l.items[id]
	l.mu.RUnlock()
	return item.Data, ok
}

func (l *LazyLoader) GetVisibleItems(visibleIDs []string) []string {
	l.mu.RLock()
	defer l.mu.RUnlock()
	var unloaded []string
	for _, id := range visibleIDs {
		if !l.loaded[id] && !l.loading[id] {
			unloaded = append(unloaded, id)
		}
	}
	return unloaded
}

func (l *LazyLoader) RequestLoad(id string) bool {
	l.mu.Lock()
	if l.loaded[id] || l.loading[id] {
		l.mu.Unlock()
		return false
	}
	if len(l.semaphore) >= l.maxConcurrent {
		l.mu.Unlock()
		return false
	}
	l.loading[id] = true
	l.semaphore <- struct{}{}
	l.mu.Unlock()

	if l.onLoad != nil {
		go func() {
			l.onLoad(id)
			<-l.semaphore
		}()
	}
	return true
}

func (l *LazyLoader) Count() (loaded, loading, pending int) {
	l.mu.RLock()
	defer l.mu.RUnlock()
	loaded = len(l.loaded)
	loading = len(l.loading)
	pending = len(l.items) - loaded - loading
	return
}

func (l *LazyLoader) Clear() {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.loaded = make(map[string]bool)
	l.loading = make(map[string]bool)
	l.items = make(map[string]LazyItem)
}

type LoadingState int

const (
	LoadingStatePending LoadingState = iota
	LoadingStateLoading
	LoadingStateLoaded
	LoadingStateError
)

type LazyLoaderV2 struct {
	items    map[string]*lazyItemData
	onLoad   func(string) any
	maxQueue int
	mu       sync.RWMutex
	cond     *sync.Cond
}

type lazyItemData struct {
	state    LoadingState
	data     any
	err      error
	refCount int
}

func NewLazyLoaderV2(maxQueue int, onLoad func(string) any) *LazyLoaderV2 {
	l := &LazyLoaderV2{
		items:    make(map[string]*lazyItemData),
		onLoad:   onLoad,
		maxQueue: maxQueue,
	}
	l.cond = sync.NewCond(&l.mu)
	return l
}

func (l *LazyLoaderV2) Get(id string) (any, LoadingState) {
	l.mu.RLock()
	item, ok := l.items[id]
	l.mu.RUnlock()
	if !ok {
		return nil, LoadingStatePending
	}
	return item.data, item.state
}

func (l *LazyLoaderV2) Request(id string) {
	l.mu.Lock()
	defer l.mu.Unlock()
	if item, ok := l.items[id]; ok {
		if item.state == LoadingStatePending {
			item.state = LoadingStateLoading
		}
		item.refCount++
	}
}

func (l *LazyLoaderV2) Release(id string) {
	l.mu.Lock()
	defer l.mu.Unlock()
	if item, ok := l.items[id]; ok {
		item.refCount--
	}
}

func (l *LazyLoaderV2) Load(ids []string) {
	l.mu.Lock()
	for _, id := range ids {
		if _, ok := l.items[id]; !ok {
			l.items[id] = &lazyItemData{state: LoadingStatePending, refCount: 0}
		}
	}
	l.mu.Unlock()
}
