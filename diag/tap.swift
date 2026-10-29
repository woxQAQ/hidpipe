// Event diagnostic: listens for mouse and keyboard events at the HID
// level and prints the type and modifier flags.
// Used to track down "left click becomes right click": does the left
// click injected by hidpipe carry Control or not.
//
// Usage (in a terminal that already has Accessibility access, run
// alongside hidpipe-client):
//   swift diag/tap.swift
// Then in remote mode: left click once, right click once, press a letter
// key, Ctrl-C to stop.
import CoreGraphics
import Foundation

let mask: CGEventMask =
    (1 << CGEventType.leftMouseDown.rawValue)
    | (1 << CGEventType.leftMouseUp.rawValue)
    | (1 << CGEventType.rightMouseDown.rawValue)
    | (1 << CGEventType.rightMouseUp.rawValue)
    | (1 << CGEventType.keyDown.rawValue)
    | (1 << CGEventType.keyUp.rawValue)
    | (1 << CGEventType.flagsChanged.rawValue)

let callback: CGEventTapCallBack = { _, type, event, _ in
    let name: String
    switch type {
    case .leftMouseDown: name = "LeftDown"
    case .leftMouseUp: name = "LeftUp"
    case .rightMouseDown: name = "RightDown"
    case .rightMouseUp: name = "RightUp"
    case .keyDown: name = "KeyDown"
    case .keyUp: name = "KeyUp"
    case .flagsChanged: name = "FlagsChanged"
    default: name = "other(\(type.rawValue))"
    }
    let flags = event.flags.rawValue
    let button = event.getIntegerValueField(.mouseEventButtonNumber)
    let keycode = event.getIntegerValueField(.keyboardEventKeycode)
    print("\(name) flags=0x\(String(flags, radix: 16)) button=\(button) key=\(keycode)")
    return Unmanaged.passUnretained(event)
}

guard
    let tap = CGEvent.tapCreate(
        tap: .cghidEventTap,
        place: .headInsertEventTap,
        options: .listenOnly,
        eventsOfInterest: mask,
        callback: callback,
        userInfo: nil
    )
else {
    print("failed to create event tap; grant this terminal access in Accessibility / Input Monitoring")
    exit(1)
}

let source = CFMachPortCreateRunLoopSource(nil, tap, 0)
CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)
print("Listening. flags: 0x40000=Control 0x20000=Shift 0x80000=Option 0x100000=Command")
RunLoop.main.run()
