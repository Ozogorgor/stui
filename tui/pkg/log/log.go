package log

import (
	"context"
	"io"
	"log/slog"
	"os"
	"runtime"
	"strconv"
	"sync/atomic"
)

type contextKey string

const loggerKey contextKey = "logger"

var defaultLogger atomic.Pointer[slog.Logger]
var logLevel = new(slog.LevelVar)

func init() {
	Setup(&Config{
		Level:  LevelInfo,
		Format: FormatText,
	})
}

type Config struct {
	Level  Level
	Format Format
}

type Level slog.Level

const (
	LevelDebug Level = Level(slog.LevelDebug)
	LevelInfo  Level = Level(slog.LevelInfo)
	LevelWarn  Level = Level(slog.LevelWarn)
	LevelError Level = Level(slog.LevelError)
)

type Format string

const (
	FormatText Format = "text"
	FormatJSON Format = "json"
)

func Setup(cfg *Config) {
	var handler slog.Handler
	opts := &slog.HandlerOptions{Level: logLevel}

	if cfg != nil {
		logLevel.Set(slog.Level(cfg.Level))
		switch cfg.Format {
		case FormatJSON:
			handler = slog.NewJSONHandler(os.Stderr, opts)
		default:
			handler = slog.NewTextHandler(os.Stderr, opts)
		}
	} else {
		handler = slog.NewTextHandler(os.Stderr, opts)
	}

	l := slog.New(handler)
	defaultLogger.Store(l)
	slog.SetDefault(l)
}

func SetOutput(w io.Writer) {
	l := slog.New(slog.NewTextHandler(w, &slog.HandlerOptions{Level: logLevel}))
	defaultLogger.Store(l)
	slog.SetDefault(l)
}

func SetLevel(lvl Level) {
	logLevel.Set(slog.Level(lvl))
}

func Logger() *slog.Logger {
	return defaultLogger.Load()
}

func WithContext(ctx context.Context) *slog.Logger {
	if logger, ok := ctx.Value(loggerKey).(*slog.Logger); ok {
		return logger
	}
	return defaultLogger.Load()
}

func WithContextValue(ctx context.Context, logger *slog.Logger) context.Context {
	return context.WithValue(ctx, loggerKey, logger)
}

func Debug(msg string, args ...any) {
	defaultLogger.Load().Debug(msg, args...)
}

func Info(msg string, args ...any) {
	defaultLogger.Load().Info(msg, args...)
}

func Warn(msg string, args ...any) {
	defaultLogger.Load().Warn(msg, args...)
}

func Error(msg string, args ...any) {
	defaultLogger.Load().Error(msg, args...)
}

func Fatal(msg string, args ...any) {
	_, file, line, ok := runtime.Caller(1)
	if ok {
		args = append(args, "caller", file+":"+strconv.Itoa(line))
	}
	defaultLogger.Load().Error(msg, args...)
	os.Exit(1)
}

type IPCLogger struct {
	logger *slog.Logger
}

func NewIPCLogger() *IPCLogger {
	return &IPCLogger{logger: defaultLogger.Load().With("component", "ipc")}
}

func (l *IPCLogger) Debug(msg string, args ...any) {
	l.logger.Debug(msg, args...)
}

func (l *IPCLogger) Info(msg string, args ...any) {
	l.logger.Info(msg, args...)
}

func (l *IPCLogger) Warn(msg string, args ...any) {
	l.logger.Warn(msg, args...)
}

func (l *IPCLogger) Error(msg string, args ...any) {
	l.logger.Error(msg, args...)
}

func (l *IPCLogger) With(args ...any) *IPCLogger {
	return &IPCLogger{logger: l.logger.With(args...)}
}
