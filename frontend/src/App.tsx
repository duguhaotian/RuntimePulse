import { useMemo, useState } from 'react';
import { createHttpRuntimePulseApi } from './api/httpRuntimePulseApi';
import { AppLayout } from './components/layout/AppLayout';
import { NodeDetail } from './pages/NodeDetail/NodeDetail';
import { SandboxExplorer } from './pages/SandboxExplorer/SandboxExplorer';
import { SandboxDetail } from './pages/SandboxDetail/SandboxDetail';
import { RuntimeComparison } from './pages/RuntimeComparison/RuntimeComparison';
import { CollectorStatus } from './pages/CollectorStatus/CollectorStatus';
import { Artifacts } from './pages/Artifacts/Artifacts';

type Page = 'explorer' | 'node' | 'detail' | 'runtime' | 'collectors' | 'artifacts';

export function App() {
  const api = useMemo(() => {
    const apiBaseUrl = import.meta.env.VITE_RUNTIMEPULSE_API_BASE_URL || '/api';
    return createHttpRuntimePulseApi(apiBaseUrl);
  }, []);
  const [page, setPage] = useState<Page>('explorer');
  const [selectedNodeId, setSelectedNodeId] = useState('');
  const [selectedSandboxId, setSelectedSandboxId] = useState('');

  return (
    <AppLayout activePage={page === 'detail' || page === 'node' ? 'explorer' : page} onNavigate={(nextPage) => setPage(nextPage)}>
      {page === 'explorer' && (
        <SandboxExplorer
          api={api}
          onSelectNode={(id) => {
            setSelectedNodeId(id);
            setPage('node');
          }}
        />
      )}
      {page === 'node' && selectedNodeId && (
        <NodeDetail
          api={api}
          nodeId={selectedNodeId}
          onBack={() => setPage('explorer')}
          onSelectSandbox={(id) => {
            setSelectedSandboxId(id);
            setPage('detail');
          }}
        />
      )}
      {page === 'detail' && selectedSandboxId && <SandboxDetail api={api} sandboxId={selectedSandboxId} onBack={() => setPage('node')} />}
      {page === 'runtime' && <RuntimeComparison api={api} />}
      {page === 'collectors' && <CollectorStatus api={api} />}
      {page === 'artifacts' && <Artifacts api={api} />}
    </AppLayout>
  );
}
