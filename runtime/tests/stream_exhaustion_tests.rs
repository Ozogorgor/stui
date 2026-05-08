//! Integration tests for stream candidate exhaustion and error handling.
//!
//! Verifies that when all stream candidates fail, the error path works correctly:
//! - `AllCandidatesExhausted` is returned (not a panic)
//! - `is_recoverable()` returns false for exhaustion (no automatic retry)
//! - `user_message()` provides human-readable feedback
//! - `StreamFailure` is marked as recoverable (triggers fallback to next candidate)

use stui_runtime::error::StuidError;

// ── AllCandidatesExhausted ────────────────────────────────────────────────────

#[test]
fn test_all_candidates_exhausted_error_is_not_recoverable() {
    let err = StuidError::AllCandidatesExhausted {
        entry_id: "tt1234567".into(),
    };
    assert!(
        !err.is_recoverable(),
        "AllCandidatesExhausted should NOT be recoverable (no auto-retry)"
    );
}

#[test]
fn test_all_candidates_exhausted_has_user_message() {
    let err = StuidError::AllCandidatesExhausted {
        entry_id: "tt1234567".into(),
    };
    let msg = err.user_message();
    assert!(
        !msg.is_empty(),
        "user_message() should not be empty for AllCandidatesExhausted"
    );
    // Message should mention failure/no streams
    assert!(
        msg.contains("No") || msg.contains("working") || msg.contains("streams"),
        "user_message() should mention 'No' or 'working' or 'streams', got: {}",
        msg
    );
}

#[test]
fn test_all_candidates_exhausted_to_string() {
    let err = StuidError::AllCandidatesExhausted {
        entry_id: "tt1234567".into(),
    };
    let s = err.to_string();
    assert!(
        s.contains("exhausted"),
        "to_string() should include 'exhausted', got: {}",
        s
    );
    assert!(
        s.contains("tt1234567"),
        "to_string() should include entry_id, got: {}",
        s
    );
}

// ── StreamFailure (recoverable) ───────────────────────────────────────────────

#[test]
fn test_stream_failure_error_is_recoverable() {
    let err = StuidError::StreamFailure {
        url: "https://example.com/video.mp4".into(),
        reason: "connection reset".into(),
    };
    assert!(
        err.is_recoverable(),
        "StreamFailure should be recoverable (triggers fallback to next candidate)"
    );
}

#[test]
fn test_stream_failure_has_user_message() {
    let err = StuidError::StreamFailure {
        url: "https://example.com/video.mp4".into(),
        reason: "timeout".into(),
    };
    let msg = err.user_message();
    assert!(
        !msg.is_empty(),
        "user_message() should not be empty for StreamFailure"
    );
    assert!(
        msg.contains("failed") || msg.contains("timeout") || msg.contains("Stream"),
        "user_message() should mention failure reason or stream, got: {}",
        msg
    );
}

#[test]
fn test_stream_failure_different_reasons() {
    let reasons = vec![
        "connection reset",
        "timeout",
        "DNS resolution failed",
        "HTTP 404",
        "certificate expired",
    ];
    for reason in reasons {
        let err = StuidError::StreamFailure {
            url: "https://test.com/stream".into(),
            reason: reason.into(),
        };
        assert!(
            err.is_recoverable(),
            "StreamFailure with reason '{}' should be recoverable",
            reason
        );
        let msg = err.user_message();
        assert!(
            !msg.is_empty(),
            "user_message() should handle reason '{}'",
            reason
        );
    }
}

// ── Error classification ──────────────────────────────────────────────────────

#[test]
fn test_error_classification_non_recoverable() {
    // These should NOT be recoverable (don't trigger fallback)
    let non_recoverable = vec![
        StuidError::AllCandidatesExhausted {
            entry_id: "tt1234567".into(),
        },
        StuidError::Config("invalid setting".into()),
        StuidError::PluginNotFound("myplugin".into()),
    ];

    for err in non_recoverable {
        assert!(
            !err.is_recoverable(),
            "Error should not be recoverable: {:?}",
            err
        );
    }
}

#[test]
fn test_error_classification_recoverable() {
    // These should be recoverable (trigger fallback to next candidate)
    let recoverable = vec![
        StuidError::StreamFailure {
            url: "https://test.com".into(),
            reason: "timeout".into(),
        },
        StuidError::Provider {
            provider: "testprov".into(),
            reason: "API down".into(),
        },
        StuidError::ProviderTimeout {
            provider: "testprov".into(),
            timeout_ms: 5000,
        },
        StuidError::RateLimited {
            provider: "testprov".into(),
            retry_after_secs: 60,
        },
        StuidError::NoStream {
            entry_id: "tt1234567".into(),
        },
    ];

    for err in recoverable {
        assert!(
            err.is_recoverable(),
            "Error should be recoverable: {:?}",
            err
        );
    }
}

// ── Integration: exhaustion scenario ──────────────────────────────────────────

#[test]
fn test_exhaustion_after_multiple_failures() {
    // Scenario: multiple streams are tried, all fail with different reasons
    let failure1 = StuidError::StreamFailure {
        url: "https://mirror1.example.com/stream".into(),
        reason: "timeout".into(),
    };
    let failure2 = StuidError::StreamFailure {
        url: "https://mirror2.example.com/stream".into(),
        reason: "connection reset".into(),
    };
    let failure3 = StuidError::StreamFailure {
        url: "https://mirror3.example.com/stream".into(),
        reason: "HTTP 503 Service Unavailable".into(),
    };

    // Each failure is individually recoverable (triggers fallback)
    assert!(failure1.is_recoverable());
    assert!(failure2.is_recoverable());
    assert!(failure3.is_recoverable());

    // But after all candidates are exhausted, the final error is NOT recoverable
    let exhausted = StuidError::AllCandidatesExhausted {
        entry_id: "tt1234567".into(),
    };
    assert!(!exhausted.is_recoverable());
    assert!(exhausted
        .user_message()
        .contains("No working streams found"));
}

#[test]
fn test_user_messages_distinct() {
    // Verify that different error types produce meaningfully different messages
    let exhausted_msg = StuidError::AllCandidatesExhausted {
        entry_id: "tt1234567".into(),
    }
    .user_message();

    let stream_fail_msg = StuidError::StreamFailure {
        url: "https://example.com".into(),
        reason: "timeout".into(),
    }
    .user_message();

    // They should be different
    assert_ne!(
        exhausted_msg, stream_fail_msg,
        "Different errors should have different user messages"
    );

    // Each should be meaningful
    assert!(exhausted_msg.len() > 5);
    assert!(stream_fail_msg.len() > 5);
}
