package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"net/http"
	"net/url"
	"os"
	"time"

	"github.com/getsentry/sentry-go"
)

type OrderValidationError struct {
	OrderID int
	Reason  string
}

func (e *OrderValidationError) Error() string {
	return fmt.Sprintf("order validation failed for order %d: %s", e.OrderID, e.Reason)
}

func processOrder(orderID int) error {
	return validateInventory(orderID)
}

func validateInventory(orderID int) error {
	return &OrderValidationError{
		OrderID: orderID,
		Reason:  "insufficient stock",
	}
}

func triggerCheckoutPanic() {
	panic("go checkout nil gateway client")
}

func runErrorAndTrace() {
	dsn := os.Getenv("SENTRY_DSN")
	if dsn == "" {
		fmt.Fprintln(os.Stderr, "SENTRY_DSN is required")
		os.Exit(2)
	}

	err := sentry.Init(sentry.ClientOptions{
		Dsn:              dsn,
		Environment:      "e2e-go",
		Release:          "e2e-go-sha-a1b2c3d",
		AttachStacktrace: true,
		EnableTracing:    true,
		TracesSampleRate: 1.0,
		SendDefaultPII:   true,
	})
	if err != nil {
		fmt.Fprintf(os.Stderr, "sentry.Init failed: %v\n", err)
		os.Exit(1)
	}
	defer sentry.Flush(10 * time.Second)

	bearerToken := os.Getenv("E2E_BEARER_TOKEN")
	cookieVal := os.Getenv("E2E_COOKIE_VALUE")

	fakeURL, _ := url.Parse("https://orders.internal/api/go/orders/1001?api_key=secret-query-key")
	fakeReq := &http.Request{
		Method: http.MethodPost,
		URL:    fakeURL,
		Header: http.Header{
			"Authorization": []string{"Bearer " + bearerToken},
			"Cookie":        []string{"session_id=" + cookieVal},
		},
	}

	// 1. Capture the same logical Go error 3 times with different order IDs -> should group into 1 issue.
	for _, orderID := range []int{1001, 1002, 1003} {
		hub := sentry.CurrentHub().Clone()
		hub.ConfigureScope(func(scope *sentry.Scope) {
			scope.SetRequest(fakeReq)
			scope.SetTag("sdk", "sentry-go")
			scope.SetTag("order_id", fmt.Sprintf("%d", orderID))
		})
		if procErr := processOrder(orderID); procErr != nil {
			hub.CaptureException(procErr)
		}
	}

	// 2. Capture a recovered panic -> separate 2nd issue with Go stack frames.
	func() {
		hub := sentry.CurrentHub().Clone()
		hub.ConfigureScope(func(scope *sentry.Scope) {
			scope.SetRequest(fakeReq)
			scope.SetTag("sdk", "sentry-go")
		})
		defer func() {
			if r := recover(); r != nil {
				hub.Recover(r)
			}
		}()
		triggerCheckoutPanic()
	}()

	// 3. Send a real Go performance transaction with 2 child spans (error status so it is 100% sampled).
	ctx := context.Background()
	tx := sentry.StartTransaction(
		ctx,
		"GET /api/go/orders/{id}",
		sentry.WithOpName("http.server"),
	)
	tx.SetTag("sdk", "sentry-go")

	dbSpan := tx.StartChild("db.sql.query", sentry.WithDescription("SELECT id, status FROM orders WHERE id = $1"))
	time.Sleep(25 * time.Millisecond)
	dbSpan.Status = sentry.SpanStatusOK
	dbSpan.Finish()

	httpSpan := tx.StartChild("http.client", sentry.WithDescription("POST https://inventory.internal/reserve"))
	time.Sleep(15 * time.Millisecond)
	httpSpan.Status = sentry.SpanStatusInternalError
	httpSpan.Finish()

	tx.Status = sentry.SpanStatusInternalError
	traceID := tx.TraceID.String()
	tx.Finish()

	if !sentry.Flush(10 * time.Second) {
		fmt.Fprintln(os.Stderr, "sentry.Flush timed out")
		os.Exit(1)
	}

	fmt.Printf("TRACE_ID=%s\n", traceID)
}

func runInvalidDsn() {
	dsn := os.Getenv("SENTRY_DSN")
	if dsn == "" {
		fmt.Fprintln(os.Stderr, "SENTRY_DSN is required")
		os.Exit(2)
	}
	err := sentry.Init(sentry.ClientOptions{
		Dsn:         dsn,
		Environment: "e2e-go-negative",
	})
	if err != nil {
		fmt.Fprintf(os.Stderr, "sentry.Init failed: %v\n", err)
		os.Exit(1)
	}
	sentry.CaptureException(errors.New("should be rejected by invalid dsn"))
	sentry.Flush(5 * time.Second)
}

func main() {
	mode := flag.String("mode", "error-and-trace", "worker mode: error-and-trace or invalid-dsn")
	flag.Parse()

	switch *mode {
	case "error-and-trace":
		runErrorAndTrace()
	case "invalid-dsn":
		runInvalidDsn()
	default:
		fmt.Fprintf(os.Stderr, "unknown mode: %s\n", *mode)
		os.Exit(2)
	}
}
