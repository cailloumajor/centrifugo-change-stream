import assert from "node:assert"
import { Centrifuge } from "npm:centrifuge@5.3"

interface Publication {
    val: {
        integer?: number
        float?: number
    }
    ts: {
        first?: string
        second?: string
    }
}

function assertInRange(n: number, min: number, max: number) {
    assert.ok(n >= min && n <= max, `unexpected ${min} <= ${n} <= ${max}`)
}
function assertIntegerRange(n: number) {
    assertInRange(n, 150, 160)
}
function assertFloatRange(n: number) {
    assertInRange(n, 32.0, 35.0)
}
function assertDate(ts: string) {
    assert.ok(!isNaN(Date.parse(ts)), `unexpected timestamp: ${ts}`)
}

if (import.meta.main) {
    let noDataSubscribed = false
    const publications: Publication[] = []

    const centrifuge = new Centrifuge("ws://centrifugo:8000/connection/websocket", {
        debug: true,
    })

    const noDataSub = centrifuge.newSubscription("testdb.testcoll:nodata")

    noDataSub.on("subscribed", ({ channel, data }) => {
        console.log(
            "got subscribed event for `%s` channel, with data: %O",
            channel,
            data,
        )
        noDataSubscribed = true
        console.log("testing no-data channel data object")
        assert.strictEqual(Object.keys(data).length, 0)
        assert.strictEqual(data.constructor, Object)
    })

    noDataSub.on("publication", () => {
        throw new Error("unexpected publication on no-data channel")
    })

    noDataSub.subscribe()

    const sub = centrifuge.newSubscription("testdb.testcoll:integration-tests")

    sub.on("subscribed", ({ channel, data }) => {
        console.log(
            "got subscribed event for `%s` channel, with data: %O",
            channel,
            data,
        )
        console.log("testing data object validity from subscribe proxy")
        assertIntegerRange(data.val.integer)
        assertFloatRange(data.val.float)
        assertDate(data.ts.first)
        assertDate(data.ts.second)
    })

    sub.on("publication", ({ data }) => {
        console.log("got publication with data: %O", data)
        publications.push(data)
    })

    sub.subscribe()

    centrifuge.connect()

    const publicationsTimeout = setTimeout(() => {
        throw new Error("timeout waiting for publications array to populate")
    }, 10000)

    await new Promise((resolve) => {
        const interval = setInterval(() => {
            if (publications.length >= 6) {
                clearInterval(interval)
                resolve(null)
            }
        }, 500)
    })

    centrifuge.disconnect()
    clearTimeout(publicationsTimeout)

    assert.ok(noDataSubscribed, "no-data channel has not been subscribed")

    console.log("testing validity of received publications")

    const integersPublished = publications.flatMap((pub) =>
        pub.val.integer != null ? [pub.val.integer] : []
    )
    assert.strictEqual(integersPublished.length, 4)
    integersPublished.forEach((val) => assertIntegerRange(val))

    const floatsPublished = publications.flatMap((pub) =>
        pub.val.float != null ? [pub.val.float] : []
    )
    assert.strictEqual(floatsPublished.length, 4)
    floatsPublished.forEach((val) => assertFloatRange(val))

    const tsFirstPublished = publications.flatMap((data) =>
        data.ts.first != null ? [data.ts.first] : []
    )
    assert.strictEqual(tsFirstPublished.length, 4)
    tsFirstPublished.forEach((val) => assertDate(val))

    const tsSecondPublished = publications.flatMap(
        (data) => data.ts.second != null ? [data.ts.second] : [],
    )
    assert.strictEqual(tsSecondPublished.length, 4)
    tsSecondPublished.forEach((val) => assertDate(val))

    console.log(`${import.meta.filename}: success`)
}
