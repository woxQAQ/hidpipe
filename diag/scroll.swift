// Observe macOS scroll events at the same HID tap where hidpipe posts them.
// Usage: swift diag/scroll.swift | tee /tmp/hidpipe-scroll.log
import CoreGraphics
import Foundation

final class ScrollMonitor {
    var tap: CFMachPort?
    var count = 0
    let started = ProcessInfo.processInfo.systemUptime
    var previous: TimeInterval?

    func record(_ event: CGEvent) {
        let now = ProcessInfo.processInfo.systemUptime
        let elapsed = String(format: "%.3f", now - started)
        let interval = previous.map { String(format: "%.2f", (now - $0) * 1000) } ?? "-"
        previous = now
        count += 1

        // Axis 1 is vertical; axis 2 is horizontal. Print pairs as (x,y).
        let lineX = event.getIntegerValueField(.scrollWheelEventDeltaAxis2)
        let lineY = event.getIntegerValueField(.scrollWheelEventDeltaAxis1)
        let pointX = event.getIntegerValueField(.scrollWheelEventPointDeltaAxis2)
        let pointY = event.getIntegerValueField(.scrollWheelEventPointDeltaAxis1)
        let fixedX = event.getDoubleValueField(.scrollWheelEventFixedPtDeltaAxis2)
        let fixedY = event.getDoubleValueField(.scrollWheelEventFixedPtDeltaAxis1)
        let fixed = String(format: "(%.4f,%.4f)", fixedX, fixedY)
        let continuous = event.getIntegerValueField(.scrollWheelEventIsContinuous)
        let phase = event.getIntegerValueField(.scrollWheelEventScrollPhase)
        let momentum = event.getIntegerValueField(.scrollWheelEventMomentumPhase)
        let pid = event.getIntegerValueField(.eventSourceUnixProcessID)
        let flags = String(event.flags.rawValue, radix: 16)
        print(
            "#\(count) t=\(elapsed)s dt=\(interval)ms timestamp_ns=\(event.timestamp) "
                + "line=(\(lineX),\(lineY)) point=(\(pointX),\(pointY)) fixed=\(fixed) "
                + "continuous=\(continuous) phase=\(phase) momentum=\(momentum) "
                + "pid=\(pid) flags=0x\(flags)"
        )
        fflush(stdout)
    }
}

let monitor = ScrollMonitor()
let callback: CGEventTapCallBack = { _, type, event, context in
    guard let context = context else { return Unmanaged.passUnretained(event) }
    let monitor = Unmanaged<ScrollMonitor>.fromOpaque(context).takeUnretainedValue()
    switch type {
    case .tapDisabledByTimeout, .tapDisabledByUserInput:
        if let tap = monitor.tap {
            CGEvent.tapEnable(tap: tap, enable: true)
            fputs("Scroll tap was disabled; re-enabled. Events may have been missed.\n", stderr)
        }
    case .scrollWheel:
        monitor.record(event)
    default:
        break
    }
    return Unmanaged.passUnretained(event)
}

// The run loop retains this top-level monitor for the lifetime of the tap.
guard let tap = CGEvent.tapCreate(
    tap: .cghidEventTap,
    place: .headInsertEventTap,
    options: .listenOnly,
    eventsOfInterest: CGEventMask(1) << CGEventType.scrollWheel.rawValue,
    callback: callback,
    userInfo: Unmanaged.passUnretained(monitor).toOpaque()
) else {
    fputs(
        "Failed to create scroll tap. Grant this terminal Input Monitoring / Accessibility "
            + "in System Settings > Privacy & Security, then restart the terminal and retry.\n",
        stderr
    )
    exit(1)
}
monitor.tap = tap

guard let source = CFMachPortCreateRunLoopSource(nil, tap, 0) else {
    fputs("Failed to create scroll tap run-loop source.\n", stderr)
    exit(1)
}
CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)
CGEvent.tapEnable(tap: tap, enable: true)
fputs(
    "Listening for scroll events at the macOS HID tap (Ctrl-C to stop).\n"
        + "Pairs are (x,y): line=integer line delta, point=pixel delta, fixed=fractional line delta.\n"
        + "dt=arrival interval; continuous=1 for pixel scrolling; phase/momentum are raw phase values.\n"
        + "pid is the event source PID (0 may mean system/hardware); flags are modifier bits.\n",
    stderr
)
RunLoop.main.run()
