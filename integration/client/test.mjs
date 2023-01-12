import { basename } from "node:path";
import { argv } from "node:process";
import { format } from "node:util";

import { Centrifuge } from "centrifuge";
import WebSocket from "ws";

const inRange = (n, min, max) => n >= min && n <= max;

let noDataSubscribed = false;
const publications = [];

const centrifuge = new Centrifuge("ws://centrifugo:8000/connection/websocket", {
    debug: true,
    websocket: WebSocket,
});

const noDataSub = centrifuge.newSubscription("testdb.testcoll:nodata");

noDataSub.on("subscribed", ({ channel, data }) => {
    console.log(
        "got subscribed event for `%s` channel, with data: %s",
        channel,
        data
    );
    noDataSubscribed = true;
    if (Object.keys(data).length !== 0 || data.constructor !== Object) {
        throw new Error("no-data channel: expected empty data object");
    }
});

noDataSub.on("publication", () => {
    throw new Error("unexpected publication on no-data channel");
});

noDataSub.subscribe();

const sub = centrifuge.newSubscription("testdb.testcoll:integration-tests");

sub.on("subscribed", ({ channel, data }) => {
    console.log(
        "got subscribed event for `%s` channel, with data: %s",
        channel,
        data
    );
    if (!inRange(data.integer, 150, 250) || !inRange(data.float, 32.0, 42.0)) {
        const errorMessage = format(
            "unexpected data from subscribe proxy: %s",
            data
        );
        throw new Error(errorMessage);
    }
});

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
        let integerCount = 0;
        let floatCount = 0;
        publications.forEach((pub) => {
            if ("integer" in pub) {
                integerCount += 1;
            }
            if ("float" in pub) {
                floatCount += 1;
            }
        });
        if (integerCount >= 3 && floatCount >= 3) {
            clearInterval(interval);
            resolve();
        }
    }, 500);
});

centrifuge.disconnect();
clearTimeout(publicationsTimeout);

if (!noDataSubscribed) {
    throw new Error("no-data channel has not been subscribed");
}

for (const data of publications) {
    if (
        (data.integer && !inRange(data.integer, 150, 250)) ||
        (data.float && !inRange(data.float, 32.0, 42.0))
    ) {
        const errorMessage = format("test failed for element: %s", data);
        throw new Error(errorMessage);
    }
}

console.log("%s: success", basename(argv[1]));
