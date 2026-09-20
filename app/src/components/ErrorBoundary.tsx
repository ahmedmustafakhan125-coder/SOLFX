import { Component, type ErrorInfo, type ReactNode } from "react";

type Props = { children: ReactNode };
type State = { error: Error | undefined; stack: string | undefined };

/**
 * Renders whatever went wrong instead of a blank page.
 *
 * A React tree that throws during render unmounts to nothing, and "the screen is black" is
 * the least useful bug report there is — it hides the error from everyone who is not already
 * looking at a browser console. This puts the message and the component stack on the page.
 */
export class ErrorBoundary extends Component<Props, State> {
  override state: State = { error: undefined, stack: undefined };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  override componentDidCatch(error: Error, info: ErrorInfo) {
    this.setState({ stack: info.componentStack ?? undefined });
    console.error("[SolFX] render failed:", error, info.componentStack);
  }

  override render() {
    const { error, stack } = this.state;
    if (!error) return this.props.children;

    return (
      <div className="min-h-screen bg-bg p-6 text-ink md:p-10">
        <div className="mx-auto max-w-3xl">
          <h1 className="text-lg font-bold text-short">
            The interface failed to render
          </h1>
          <p className="mt-2 text-xs text-ink-muted">
            The error is below. Everything on chain is unaffected. This is a bug
            in the page, not in the protocol.
          </p>

          <div className="mt-5 rounded-lg border border-short/40 bg-short/5 p-4">
            <div className="text-[10px] uppercase tracking-[0.16em] text-ink-dim">
              Error
            </div>
            <pre className="tnum mt-1 whitespace-pre-wrap break-words text-xs text-short">
              {error.name}: {error.message}
            </pre>
          </div>

          {error.stack ? (
            <details
              open
              className="mt-3 rounded-lg border border-line-soft bg-surface p-4"
            >
              <summary className="cursor-pointer text-[10px] uppercase tracking-[0.16em] text-ink-dim">
                Stack
              </summary>
              <pre className="tnum mt-2 max-h-64 overflow-auto whitespace-pre-wrap break-words text-[10px] leading-snug text-ink-muted">
                {error.stack}
              </pre>
            </details>
          ) : null}

          {stack ? (
            <details className="mt-3 rounded-lg border border-line-soft bg-surface p-4">
              <summary className="cursor-pointer text-[10px] uppercase tracking-[0.16em] text-ink-dim">
                Component tree
              </summary>
              <pre className="tnum mt-2 max-h-64 overflow-auto whitespace-pre-wrap text-[10px] leading-snug text-ink-muted">
                {stack}
              </pre>
            </details>
          ) : null}

          <button
            onClick={() => window.location.reload()}
            className="mt-5 rounded-md bg-brand px-4 py-2 text-xs font-semibold text-white hover:bg-brand-dim"
          >
            Reload
          </button>
        </div>
      </div>
    );
  }
}
