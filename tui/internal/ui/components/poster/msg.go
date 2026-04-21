// tui/internal/ui/components/poster/msg.go
package poster

// PostersUpdatedMsg is dispatched by the long-lived `PollRefresh` Cmd each
// time the pool reports at least one newly-cached poster since the last
// receive. It's a pure "re-render please" signal; no payload.
//
// Browse-tab-owning models should recognise it and re-arm the poll Cmd:
//
//	case poster.PostersUpdatedMsg:
//	    return m, poster.PollRefresh()
type PostersUpdatedMsg struct{}
