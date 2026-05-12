FROM node:22-alpine

WORKDIR /app

COPY package.json ./
COPY src ./src

EXPOSE 8081

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD wget -qO- http://127.0.0.1:8081/health >/dev/null || exit 1

CMD ["npm", "start"]
