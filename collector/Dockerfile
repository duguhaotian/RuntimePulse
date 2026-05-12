FROM node:22-alpine

WORKDIR /app

COPY package.json ./
COPY src ./src

ENV INGEST_URL=http://runtimepulse-query-api:8081/api/ingest/batch
ENV COLLECTOR_NODE_ID=node-a
ENV COLLECTOR_INTERVAL_MS=5000

CMD ["npm", "start"]
