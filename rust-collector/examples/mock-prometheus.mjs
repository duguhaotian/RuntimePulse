import { createServer } from 'node:http';

const port = Number(process.env.PORT ?? 19090);

const samples = {
  container_cpu_usage_seconds_total: ['0.42'],
  container_memory_working_set_bytes: ['73400320'],
  container_network_receive_bytes_total: ['81920'],
  container_network_transmit_bytes_total: ['65536'],
  container_fs_reads_bytes_total: ['32768'],
  container_fs_writes_bytes_total: ['16384'],
};

createServer((request, response) => {
  const url = new URL(request.url ?? '/', `http://${request.headers.host ?? 'localhost'}`);
  if (url.pathname !== '/api/v1/query') {
    response.writeHead(404, { 'content-type': 'application/json' });
    response.end(JSON.stringify({ status: 'error', error: 'not_found' }));
    return;
  }

  const query = url.searchParams.get('query') ?? '';
  const metricName = Object.keys(samples).find((name) => query.includes(name));
  const value = metricName ? samples[metricName][0] : '0';
  response.writeHead(200, { 'content-type': 'application/json' });
  response.end(JSON.stringify({
    status: 'success',
    data: {
      resultType: 'vector',
      result: [{
        metric: {
          namespace: 'default',
          pod: 'runtimepulse-demo',
          container: metricName?.includes('network') ? 'pod' : 'app',
          node: 'kind-worker',
          image: 'registry.local/runtimepulse/demo:v1',
        },
        value: [1779340100, value],
      }],
    },
  }));
}).listen(port, () => {
  console.log(`mock prometheus listening on ${port}`);
});
