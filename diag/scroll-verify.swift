// Listen-only observation of one real client's Quartz scroll events.
import CoreGraphics
import Foundation

let help = """
scroll-verify: observe one PID at the macOS HID tap; never inject input.
Usage: swift diag/scroll-verify.swift --pid <client-pid> --axis <x|y>
  (--expected-count <0..1000000> --expected-fixed <signed-total-lines>
   | --expected-point <signed-total-points> --expected-continuous <0|1> [--expected-count <0..1000000>])
  --timeout-seconds <seconds> (default 10, must be > 0.5 and <= 3600)
  --help  print help before creating a tap (no permissions needed)
PID, axis, and exactly one expectation mode are required. Use the executable PID.
Point mode without a count observes the entire timeout; wire count is not event count.
Wait for LISTENING before pressing Enter in scroll_fixture.
Positive counts: observe for another 0.5 s after reaching the count, within timeout.
Count zero: observe the entire timeout, even when expected-fixed is zero.
Emits per-event JSON and a final JSON summary; exit 0 only on PASS.
"""

struct ArgumentError: Error, CustomStringConvertible {
    let description: String
}

struct Options {
    let pid: Int64
    let axis: String
    let count: Int?
    let fixed: Double?
    let point: Int64?
    let continuous: Int64?
    let timeout: TimeInterval

    static func parse() throws -> Options {
        let args = Array(CommandLine.arguments.dropFirst())
        if args.contains("--help") || args.contains("-h") {
            print(help)
            exit(0)
        }
        let allowed: Set<String> = ["--pid", "--axis", "--expected-count", "--expected-fixed", "--expected-point", "--expected-continuous", "--timeout-seconds"]
        var values: [String: String] = [:]
        var index = 0
        while index < args.count {
            let flag = args[index]
            guard allowed.contains(flag), values[flag] == nil, index + 1 < args.count else {
                throw ArgumentError(description: "Unknown, duplicate, or incomplete option: \(flag)")
            }
            values[flag] = args[index + 1]
            index += 2
        }
        guard let pid = Int64(values["--pid"] ?? ""), pid > 0, pid <= Int64(Int32.max),
              let axis = values["--axis"], ["x", "y"].contains(axis),
              let timeout = Double(values["--timeout-seconds"] ?? "10"), timeout.isFinite,
              timeout > 0.5, timeout <= 3600 else {
            throw ArgumentError(description: "Invalid or missing value; see --help")
        }
        let count = values["--expected-count"].flatMap(Int.init)
        if values["--expected-count"] != nil && (count == nil || !(0...1_000_000).contains(count!)) {
            throw ArgumentError(description: "Invalid expected count")
        }
        let fixed = values["--expected-fixed"].flatMap(Double.init)
        let point = values["--expected-point"].flatMap(Int64.init)
        let continuous = values["--expected-continuous"].flatMap(Int64.init)
        let fixedMode = values["--expected-fixed"] != nil
        if fixedMode {
            guard let fixed = fixed, fixed.isFinite, count != nil,
                  values["--expected-point"] == nil, values["--expected-continuous"] == nil else {
                throw ArgumentError(description: "Fixed mode requires count and finite fixed total only")
            }
        } else {
            guard point != nil, let continuous = continuous, [0, 1].contains(continuous) else {
                throw ArgumentError(description: "Point mode requires integer point total and continuous 0 or 1")
            }
        }
        return Options(pid: pid, axis: axis, count: count, fixed: fixed, point: point, continuous: continuous, timeout: timeout)
    }
}

func emit(_ value: [String: Any]) {
    do {
        let data = try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
        print(String(decoding: data, as: UTF8.self))
        fflush(stdout)
    } catch {
        fputs("Cannot encode diagnostic JSON: \(error)\n", stderr)
        exit(1)
    }
}

final class ScrollVerifier {
    let options: Options
    var tap: CFMachPort?
    var started = ProcessInfo.processInfo.systemUptime
    var reached: TimeInterval?
    var count = 0
    var fixedX = 0.0
    var fixedY = 0.0
    var lineX: Int64 = 0
    var lineY: Int64 = 0
    var pointX: Int64 = 0
    var pointY: Int64 = 0
    var finished = false
    var failure: String?
    let observationSeconds = 0.5

    init(_ options: Options) { self.options = options }

    func record(_ event: CGEvent) {
        guard !finished else { return }
        guard event.getIntegerValueField(.eventSourceUnixProcessID) == options.pid else { return }
        let x = event.getDoubleValueField(.scrollWheelEventFixedPtDeltaAxis2)
        let y = event.getDoubleValueField(.scrollWheelEventFixedPtDeltaAxis1)
        guard x.isFinite, y.isFinite else {
            finish("non-finite fixed field")
            return
        }
        let lx = event.getIntegerValueField(.scrollWheelEventDeltaAxis2)
        let ly = event.getIntegerValueField(.scrollWheelEventDeltaAxis1)
        let px = event.getIntegerValueField(.scrollWheelEventPointDeltaAxis2)
        let py = event.getIntegerValueField(.scrollWheelEventPointDeltaAxis1)
        count += 1
        fixedX += x
        fixedY += y
        lineX += lx
        lineY += ly
        pointX += px
        pointY += py
        let continuous = event.getIntegerValueField(.scrollWheelEventIsContinuous)
        if let expected = options.continuous, continuous != expected {
            finish("continuous field mismatch")
            return
        }
        if options.axis == "x" ? (py != 0) : (px != 0), options.point != nil {
            finish("unselected-axis point delta is not zero")
            return
        }
        let now = ProcessInfo.processInfo.systemUptime
        emit([
            "type": "event", "count": count, "pid": options.pid, "elapsedSeconds": now - started,
            "lineX": lx, "lineY": ly, "pointX": px, "pointY": py, "fixedX": x, "fixedY": y,
            "continuous": event.getIntegerValueField(.scrollWheelEventIsContinuous),
            "phase": event.getIntegerValueField(.scrollWheelEventScrollPhase),
            "momentum": event.getIntegerValueField(.scrollWheelEventMomentumPhase),
            "cumulativeFixedX": fixedX, "cumulativeFixedY": fixedY,
            "cumulativeLineX": lineX, "cumulativeLineY": lineY,
            "cumulativePointX": pointX, "cumulativePointY": pointY,
        ])
        if let expected = options.count {
            if expected > 0 && count == expected { reached = now }
            if count > expected { finish("extra matching scroll event") }
        }
    }

    func poll() {
        guard !finished else { return }
        let now = ProcessInfo.processInfo.systemUptime
        if let expected = options.count, expected > 0, let reached = reached,
           now >= reached + observationSeconds, reached + observationSeconds <= started + options.timeout {
            finish(nil)
        } else if now >= started + options.timeout {
            finish(options.count == nil || options.count == 0 ? nil : "timeout before count and observation window completed")
        }
    }

    func finish(_ reason: String?) {
        guard !finished else { return }
        finished = true
        let selected = options.axis == "x" ? fixedX : fixedY
        let other = options.axis == "x" ? fixedY : fixedX
        let tolerance = Double(max(1, count)) / 65536.0
        var reasons: [String] = []
        if let reason = reason { reasons.append(reason) }
        if let expected = options.count, count != expected { reasons.append("event count mismatch") }
        if let fixed = options.fixed {
            if fixed != 0 && count == 0 { reasons.append("nonzero expectation received no events") }
            if abs(selected - fixed) > tolerance { reasons.append("selected-axis fixed total mismatch") }
            if other != 0 { reasons.append("unselected-axis fixed total is not zero") }
        }
        if let point = options.point {
            let selectedPoint = options.axis == "x" ? pointX : pointY
            let otherPoint = options.axis == "x" ? pointY : pointX
            if selectedPoint != point { reasons.append("selected-axis point total mismatch") }
            if otherPoint != 0 { reasons.append("unselected-axis point total is not zero") }
            if point != 0 && count == 0 { reasons.append("nonzero expectation received no events") }
        }
        let pass = reasons.isEmpty
        if !pass { failure = reasons.joined(separator: "; ") }
        emit([
            "type": "summary", "pid": options.pid, "axis": options.axis,
            "count": count, "fixedX": fixedX, "fixedY": fixedY,
            "lineX": lineX, "lineY": lineY, "pointX": pointX, "pointY": pointY,
            "mode": options.point == nil ? "fixed" : "point",
            "expectedCount": options.count as Any? ?? NSNull(),
            "expectedFixed": options.fixed as Any? ?? NSNull(),
            "expectedPoint": options.point as Any? ?? NSNull(),
            "expectedContinuous": options.continuous as Any? ?? NSNull(), "tolerance": tolerance,
            "elapsedSeconds": ProcessInfo.processInfo.systemUptime - started,
            "observationSeconds": observationSeconds, "timeoutSeconds": options.timeout,
            "pass": pass, "reasons": reasons,
        ])
        CFRunLoopStop(CFRunLoopGetMain())
    }
}

let options: Options
do {
    options = try Options.parse()
} catch {
    fputs("\(error)\n\(help)\n", stderr)
    exit(2)
}
let monitor = ScrollVerifier(options)
let callback: CGEventTapCallBack = { _, type, event, context in
    guard let context = context else { return Unmanaged.passUnretained(event) }
    let monitor = Unmanaged<ScrollVerifier>.fromOpaque(context).takeUnretainedValue()
    switch type {
    case .tapDisabledByTimeout, .tapDisabledByUserInput:
        if let tap = monitor.tap { CGEvent.tapEnable(tap: tap, enable: true) }
        monitor.finish("tap disabled; re-enabled, but events may have been missed")
    case .scrollWheel:
        monitor.record(event)
    default:
        break
    }
    return Unmanaged.passUnretained(event)
}

guard let tap = CGEvent.tapCreate(
    tap: .cghidEventTap, place: .headInsertEventTap, options: .listenOnly,
    eventsOfInterest: CGEventMask(1) << CGEventType.scrollWheel.rawValue,
    callback: callback, userInfo: Unmanaged.passUnretained(monitor).toOpaque()
) else {
    fputs("Failed to create HID tap. Grant this terminal Input Monitoring / Accessibility in System Settings > Privacy & Security, restart it, and retry.\n", stderr)
    monitor.finish("HID tap creation failed; check permissions")
    exit(1)
}
monitor.tap = tap
guard let source = CFMachPortCreateRunLoopSource(nil, tap, 0) else {
    monitor.finish("tap run-loop source creation failed")
    exit(1)
}
CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)
CGEvent.tapEnable(tap: tap, enable: true)
guard CGEvent.tapIsEnabled(tap: tap) else {
    monitor.finish("tap is not enabled")
    exit(1)
}
monitor.started = ProcessInfo.processInfo.systemUptime
let timer = Timer(timeInterval: 0.02, repeats: true) { _ in monitor.poll() }
RunLoop.main.add(timer, forMode: .common)
fputs("LISTENING pid=\(options.pid) axis=\(options.axis); now press Enter in scroll_fixture.\n", stderr)
fflush(stderr)
CFRunLoopRun()
timer.invalidate()
CGEvent.tapEnable(tap: tap, enable: false)
if !monitor.finished { monitor.finish("run loop stopped unexpectedly") }
exit(monitor.failure == nil ? 0 : 1)
