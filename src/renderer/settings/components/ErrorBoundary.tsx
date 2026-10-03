import { Component, type ErrorInfo, type ReactNode } from 'react';
import { Button } from './Button';
import { Notice } from './Notice';

/** Keeps one broken page from blanking the whole window. */
export class ErrorBoundary extends Component<
  { children: ReactNode; resetKey?: string },
  { error: Error | null }
> {
  override state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  override componentDidCatch(error: Error, info: ErrorInfo) {
    console.error('Settings page crashed', error, info.componentStack);
  }

  override componentDidUpdate(prev: { resetKey?: string }) {
    if (prev.resetKey !== this.props.resetKey && this.state.error) this.setState({ error: null });
  }

  override render() {
    if (!this.state.error) return this.props.children;
    return (
      <Notice
        tone="danger"
        className="mt-6"
        action={
          <Button size="sm" onClick={() => this.setState({ error: null })}>
            Try again
          </Button>
        }
      >
        This page ran into a problem and could not be shown: {this.state.error.message}
      </Notice>
    );
  }
}
