FROM oven/bun:1.3.13-alpine AS build
WORKDIR /app

COPY package.json bun.lock ./
RUN bun install --frozen-lockfile
COPY . .
RUN bun run build

FROM oven/bun:1.3.13-alpine AS runtime
WORKDIR /app
ENV NODE_ENV=production
ENV HOST=0.0.0.0
ENV PORT=3000
COPY --from=build /app/build ./build
COPY --from=build /app/package.json ./package.json
EXPOSE 3000
CMD ["bun", "build/index.js"]
