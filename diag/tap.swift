// 事件诊断:在 HID 层监听鼠标/键盘事件,打印类型与修饰键标志。
// 用于定位"左键变右键"——看 hidpipe 注入的左键点击到底带不带 Control。
//
// 用法(在已授权辅助功能的终端里,与 hidpipe-client 同时运行):
//   swift diag/tap.swift
// 然后在远程模式下:左键点一次、右键点一次、按一个字母键,Ctrl-C 结束。
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
    print("无法创建事件监听——请在 辅助功能/输入监听 中授权本终端")
    exit(1)
}

let source = CFMachPortCreateRunLoopSource(nil, tap, 0)
CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)
print("监听中。flags 含义: 0x40000=Control 0x20000=Shift 0x80000=Option 0x100000=Command")
RunLoop.main.run()
