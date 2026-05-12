import type { ReactNode } from 'react';

type AppLayoutProps = {
  activePage: string;
  onNavigate: (page: 'explorer' | 'runtime' | 'collectors') => void;
  children: ReactNode;
};

const pageMeta: Record<string, { title: string; scope: string }> = {
  explorer: { title: 'Clusters', scope: 'sandbox-lab' },
  runtime: { title: 'Reports', scope: 'sandbox-lab' },
  collectors: { title: 'Collectors', scope: 'sandbox-lab' },
};

export function AppLayout({ activePage, onNavigate, children }: AppLayoutProps) {
  const meta = pageMeta[activePage] ?? pageMeta.explorer;

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark">RP</div>
          <div>
            <h1>RuntimePulse</h1>
            <p>observability workspace</p>
          </div>
        </div>

        <div className="workspace-switcher">
          <span>Team / Project</span>
          <strong>runtimepulse / sandbox-lab</strong>
        </div>

        <nav className="nav" aria-label="Primary navigation">
          <button className={activePage === 'explorer' ? 'active' : ''} onClick={() => onNavigate('explorer')}>
            <span className="nav-icon">▦</span>
            <span>Clusters</span>
          </button>
          <button className={activePage === 'runtime' ? 'active' : ''} onClick={() => onNavigate('runtime')}>
            <span className="nav-icon">◫</span>
            <span>Reports</span>
          </button>
          <button className={activePage === 'collectors' ? 'active' : ''} onClick={() => onNavigate('collectors')}>
            <span className="nav-icon">⇄</span>
            <span>Collectors</span>
          </button>
          <button disabled>
            <span className="nav-icon">◇</span>
            <span>Artifacts</span>
          </button>
          <button disabled>
            <span className="nav-icon">↯</span>
            <span>Launch queue</span>
          </button>
        </nav>

        <div className="sidebar-section">
          <p>Saved Views</p>
          <span>Slow startup runs</span>
          <span>Runtime overhead</span>
          <span>Image unpack latency</span>
          <span>Node IO pressure</span>
        </div>

        <div className="sidebar-note">
          <strong>Mock source</strong>
          <span>Phase 1 uses scenario telemetry. Collectors stay out of this frontend loop.</span>
        </div>
      </aside>

      <div className="content-shell">
        <header className="topbar">
          <div className="topbar-title">
            <p className="breadcrumb">Projects / {meta.scope} / {meta.title}</p>
            <h1>{meta.title}</h1>
          </div>
          <div className="topbar-actions">
            <div className="topbar-search">⌘K&nbsp;&nbsp;Search clusters, nodes, sandboxes</div>
            <button className="topbar-button">Last 1h</button>
            <button className="topbar-button primary">Compare</button>
            <span className="data-pill">Mock telemetry</span>
          </div>
        </header>
        <main className="main-panel">{children}</main>
      </div>
    </div>
  );
}
