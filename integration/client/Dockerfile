FROM node:22.14

USER node
WORKDIR /home/node/app

COPY package.json test.mjs ./

RUN yarn install --no-lockfile && yarn cache clean

CMD ["node", "test.mjs"]
