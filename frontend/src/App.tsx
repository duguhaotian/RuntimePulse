import { useMemo, useState } from 'react';
import { mockRuntimePulseApi } from './api/mockRuntimePulseApi';
import { AppLayout } from './components/layout/AppLayout';
import { SandboxExplorer } from './pages/SandboxExplorer/SandboxExplorer';
import { SandboxDetail } from './pages/SandboxDetail/SandboxDetail';
import { RuntimeComparison } from './pages/RuntimeComparison/RuntimeComparison';

type Page = 'explorer' | 'detail' | 'runtime';

export function App() {
  const api = useMemo(() => mockRuntimePulseApi, []);
  const [page, setPage] = useState<Page>('explorer');
  const [selectedSandboxId, setSelectedSandboxId] = useState('sb-kata-044');

  return (
    <AppLayout activePage={page === 'detail' ? 'explorer' : page} onNavigate={(nextPage) => setPage(nextPage)}>
      {page === 'explorer' && (
        <SandboxExplorer
          api={api}
          onSelectSandbox={(id) => {
            setSelectedSandboxId(id);
            setPage('detail');
          }}
        />
      )}
      {page === 'detail' && <SandboxDetail api={api} sandboxId={selectedSandboxId} onBack={() => setPage('explorer')} />}
      {page === 'runtime' && <RuntimeComparison api={api} />}
    </AppLayout>
  );
}
