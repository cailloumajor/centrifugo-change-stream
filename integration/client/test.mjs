import assert from "node:assert";
import { basename } from "node:path";
import { argv } from "node:process";

import { Centrifuge } from "centrifuge";
import WebSocket from "ws";

const inRange = (n, min, max) => n >= min && n <= max;
const validDate = (ts) => !isNaN(Date.parse(ts));

const checkData = (data) => {
    assert.ok(data.val?.integer != null);
    assert.ok(inRange(data.val.integer, 150, 250));

    assert.ok(data.val?.float != null);
    assert.ok(inRange(data.val.float, 32.0, 42.0));

    assert.ok(data.ts?.first != null);
    assert.ok(validDate(data.ts.first));

    assert.ok(data.ts?.second != null);
    assert.ok(validDate(data.ts.second));
};

let noDataSubscribed = false;
const publications = [];

const centrifuge = new Centrifuge("ws://centrifugo:8000/connection/websocket", {
    debug: true,
    websocket: WebSocket,
});

const noDataSub = centrifuge.newSubscription("testdb.testcoll:nodata");

noDataSub.on("subscribed", ({ channel, data }) => {
    console.log(
        "got subscribed event for `%s` channel, with data: %O",
        channel,
        data
    );
    noDataSubscribed = true;
    console.log("testing no-data channel data object");
    assert.strictEqual(Object.keys(data).length, 0);
    assert.strictEqual(data.constructor, Object);
});

noDataSub.on("publication", () => {
    throw new Error("unexpected publication on no-data channel");
});

noDataSub.subscribe();

const sub = centrifuge.newSubscription("testdb.testcoll:integration-tests");

sub.on("subscribed", ({ channel, data }) => {
    console.log(
        "got subscribed event for `%s` channel, with data: %O",
        channel,
        data
    );
    console.log("testing data object validity from subscribe proxy");
    checkData(data);
});

sub.on("publication", ({ data }) => {
    console.log("got publication with data: %O", data);
    publications.push(data);
});

sub.subscribe();

centrifuge.connect();

const publicationsTimeout = setTimeout(() => {
    throw new Error("timeout waiting for publications array to populate");
}, 10000);

await new Promise((resolve) => {
    const interval = setInterval(() => {
        let membersCount = [0, 0, 0, 0];
        publications.forEach((pub) => {
            if (pub.val?.integer) {
                membersCount[0] += 1;
            }
            if (pub.val?.float) {
                membersCount[1] += 1;
            }
            if (pub.ts?.first) {
                membersCount[2] += 1;
            }
            if (pub.ts?.second) {
                membersCount[3] += 1;
            }
        });
        if (!membersCount.some((count) => count < 5)) {
            clearInterval(interval);
            resolve();
        }
    }, 500);
});

centrifuge.disconnect();
clearTimeout(publicationsTimeout);

assert.ok(noDataSubscribed, "no-data channel has not been subscribed");

publications.forEach((publication) => {
    console.log("testing data object validity from received publication");
    checkData(publication);
});

console.log("%s: success", basename(argv[1]));
