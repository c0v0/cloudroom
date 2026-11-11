import { Component } from "react";
import type { ErrorInfo, ReactNode } from "react";

interface Props {
  children: ReactNode;
}

interface State {
  error: Error | null;
}

/** 顶层错误边界：运行时异常时显示可恢复提示，而不是整页空白。 */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("CloudRoom render error", error, info);
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div className="page center">
        <div className="card fatal-error">
          <h1>页面加载失败</h1>
          <p className="hint">前端遇到异常，请刷新页面重试。</p>
          <pre>{this.state.error.message}</pre>
          <button onClick={() => window.location.reload()}>刷新页面</button>
        </div>
      </div>
    );
  }
}
