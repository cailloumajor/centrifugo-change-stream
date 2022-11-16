import { basename } from "node:path";
import { argv } from "node:process";
import { format } from "node:util";

import { Centrifuge } from "centrifuge";
import WebSocket from "ws";

const publications = [];

const centrifuge = new Centrifuge("ws://centrifugo:8000/connection/websocket", {
    debug: true,
    websocket: WebSocket,
});

const sub = centrifuge.newSubscription("testdb-testcoll:integration-tests");

sub.on("publication", ({ data }) => {
    console.log("got publication with data: %s", data);
    publications.push(data);
});

sub.subscribe();

centrifuge.connect();

const publicationsTimeout = setTimeout(() => {
    throw new Error("timeout waiting for publications array to populate");
}, 10000);

await new Promise((resolve) => {
    const interval = setInterval(() => {
        if (publications.length >= 5) {
            clearInterval(interval);
            resolve();
        }
    }, 500);
});

centrifuge.disconnect();
clearTimeout(publicationsTimeout);

const inRange = (n, min, max) => n >= min && n <= max;

for (const data of publications) {
    if (!inRange(data.integer, 150, 250) || !inRange(data.float, 32.0, 42.0)) {
        const errorMessage = format("test failed for element: %s", data);
        throw new Error(errorMessage);
    }
}

console.log("%s: success", basename(argv[1]));
