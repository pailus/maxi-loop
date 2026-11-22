// Sentry-style onboarding: platform catalogue + SDK install/configure/verify snippets.
// Platform ids are free-form on the server (validated as [a-z0-9-_]{1,64}).

export type PlatformCategory = "backend" | "frontend" | "other";

export interface OnboardingPlatform {
  id: string;
  label: string;
  language: string;
  category: PlatformCategory;
}

export const ONBOARDING_PLATFORMS: OnboardingPlatform[] = [
  { id: "python", label: "Python", language: "Python", category: "backend" },
  { id: "python-fastapi", label: "FastAPI", language: "Python", category: "backend" },
  { id: "python-django", label: "Django", language: "Python", category: "backend" },
  { id: "python-flask", label: "Flask", language: "Python", category: "backend" },
  { id: "go", label: "Go", language: "Go", category: "backend" },
  { id: "rust", label: "Rust", language: "Rust", category: "backend" },
  { id: "node", label: "Node.js", language: "JavaScript", category: "backend" },
  { id: "node-express", label: "Express", language: "JavaScript", category: "backend" },
  { id: "javascript-react", label: "React", language: "JavaScript", category: "frontend" },
  { id: "javascript", label: "Browser JavaScript", language: "JavaScript", category: "frontend" },
  { id: "other", label: "Other", language: "Any Sentry SDK", category: "other" },
];

export const PLATFORM_CATEGORY_LABELS: Record<PlatformCategory | "all", string> = {
  all: "All",
  backend: "Backend",
  frontend: "Browser",
  other: "Other",
};

export interface SdkSnippetStep {
  title: string;
  description?: string;
  code: string;
}

export function findPlatform(id: string): OnboardingPlatform {
  return ONBOARDING_PLATFORMS.find((p) => p.id === id) ?? ONBOARDING_PLATFORMS[ONBOARDING_PLATFORMS.length - 1];
}

function pythonInit(dsn: string, integration: string | null): string {
  const extra = integration ? `\n    # ${integration} integration is enabled automatically` : "";
  return `import sentry_sdk

sentry_sdk.init(
    dsn="${dsn}",${extra}
    environment="production",
    traces_sample_rate=1.0,
    send_default_pii=False,
)`;
}

/** Install → Configure → Verify steps for the chosen platform, filled with the project DSN. */
export function buildSdkSteps(platformId: string, dsn: string): SdkSnippetStep[] {
  switch (platformId) {
    case "python":
      return [
        { title: "Install", code: "pip install --upgrade sentry-sdk" },
        { title: "Configure SDK", description: "Initialize as early as possible in your application.", code: pythonInit(dsn, null) },
        { title: "Verify", description: "Raise a test error to send your first event.", code: "division_by_zero = 1 / 0" },
      ];
    case "python-fastapi":
      return [
        { title: "Install", code: 'pip install --upgrade "sentry-sdk[fastapi]"' },
        { title: "Configure SDK", description: "Call init before creating the FastAPI app.", code: `${pythonInit(dsn, "FastAPI")}\n\napp = FastAPI()` },
        {
          title: "Verify",
          description: "Add this route, then open /sentry-debug in your browser.",
          code: `@app.get("/sentry-debug")
async def trigger_error():
    division_by_zero = 1 / 0`,
        },
      ];
    case "python-django":
      return [
        { title: "Install", code: 'pip install --upgrade "sentry-sdk[django]"' },
        { title: "Configure SDK", description: "Add to settings.py.", code: pythonInit(dsn, "Django") },
        {
          title: "Verify",
          description: "Add this view to urls.py, then open /sentry-debug/.",
          code: `from django.urls import path

def trigger_error(request):
    division_by_zero = 1 / 0

urlpatterns = [
    path("sentry-debug/", trigger_error),
    # ...
]`,
        },
      ];
    case "python-flask":
      return [
        { title: "Install", code: 'pip install --upgrade "sentry-sdk[flask]"' },
        { title: "Configure SDK", description: "Call init before creating the Flask app.", code: `${pythonInit(dsn, "Flask")}\n\napp = Flask(__name__)` },
        {
          title: "Verify",
          description: "Add this route, then open /debug-sentry.",
          code: `@app.route("/debug-sentry")
def trigger_error():
    division_by_zero = 1 / 0`,
        },
      ];
    case "go":
      return [
        { title: "Install", code: "go get github.com/getsentry/sentry-go" },
        {
          title: "Configure SDK",
          code: `err := sentry.Init(sentry.ClientOptions{
    Dsn:              "${dsn}",
    Environment:      "production",
    EnableTracing:    true,
    TracesSampleRate: 1.0,
})
if err != nil {
    log.Fatalf("sentry.Init: %s", err)
}
// Flush buffered events before the program terminates.
defer sentry.Flush(2 * time.Second)`,
        },
        { title: "Verify", description: "Capture a test error.", code: `sentry.CaptureException(errors.New("my first maxi-loop error"))` },
      ];
    case "rust":
      return [
        { title: "Install", description: "Add to Cargo.toml.", code: `[dependencies]\nsentry = "0.34"` },
        {
          title: "Configure SDK",
          description: "Keep the guard alive for the whole program.",
          code: `let _guard = sentry::init((
    "${dsn}",
    sentry::ClientOptions {
        release: sentry::release_name!(),
        environment: Some("production".into()),
        traces_sample_rate: 1.0,
        ..Default::default()
    },
));`,
        },
        { title: "Verify", description: "Capture a test message (or panic!).", code: `sentry::capture_message("my first maxi-loop event", sentry::Level::Error);` },
      ];
    case "node":
    case "node-express": {
      const express = platformId === "node-express";
      return [
        { title: "Install", code: "npm install --save @sentry/node" },
        {
          title: "Configure SDK",
          description: "Create instrument.js and require it before anything else.",
          code: `// instrument.js
const Sentry = require("@sentry/node");

Sentry.init({
  dsn: "${dsn}",
  environment: "production",
  tracesSampleRate: 1.0,
});`,
        },
        express
          ? {
              title: "Verify",
              description: "Register the error handler after all routes, then open /debug-sentry.",
              code: `require("./instrument");
const Sentry = require("@sentry/node");
const express = require("express");
const app = express();

app.get("/debug-sentry", () => {
  throw new Error("My first maxi-loop error!");
});

Sentry.setupExpressErrorHandler(app);
app.listen(3000);`,
            }
          : {
              title: "Verify",
              code: `require("./instrument");
const Sentry = require("@sentry/node");

try {
  foo();
} catch (e) {
  Sentry.captureException(e);
}`,
            },
      ];
    }
    case "javascript-react":
      return [
        { title: "Install", code: "npm install --save @sentry/react" },
        {
          title: "Configure SDK",
          description: "Initialize before rendering your app (e.g. in main.tsx).",
          code: `import * as Sentry from "@sentry/react";

Sentry.init({
  dsn: "${dsn}",
  environment: "production",
  integrations: [Sentry.browserTracingIntegration()],
  tracesSampleRate: 1.0,
});`,
        },
        {
          title: "Verify",
          description: "Render this button and click it.",
          code: `<button onClick={() => { throw new Error("This is your first error!"); }}>
  Break the world
</button>`,
        },
      ];
    case "javascript":
      return [
        { title: "Install", code: "npm install --save @sentry/browser" },
        {
          title: "Configure SDK",
          code: `import * as Sentry from "@sentry/browser";

Sentry.init({
  dsn: "${dsn}",
  environment: "production",
  tracesSampleRate: 1.0,
});`,
        },
        { title: "Verify", description: "Call an undefined function.", code: "myUndefinedFunction();" },
      ];
    default:
      return [
        {
          title: "Configure SDK",
          description: "Any official Sentry SDK works — point its DSN option at this project.",
          code: `SENTRY_DSN=${dsn}`,
        },
        { title: "Verify", description: "Trigger an unhandled error or capture a test message from your app." , code: "# e.g. Sentry.captureMessage(\"my first maxi-loop event\")" },
      ];
  }
}
