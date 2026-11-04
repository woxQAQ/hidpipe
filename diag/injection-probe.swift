// Passive Quartz probe: create events but never post or move the cursor.
import CoreGraphics
import Foundation

let source = CGEventSource(stateID: .combinedSessionState)!
let names = ["main_display", "bounds", "create", "configure", "drop"]
var totals = [Double](repeating: 0, count: names.count)
var maxima = totals
let count = 2500
let start = DispatchTime.now().uptimeNanoseconds

for sequence in 1...count {
    var marks = [DispatchTime.now().uptimeNanoseconds]
    let display = CGMainDisplayID()
    marks.append(DispatchTime.now().uptimeNanoseconds)
    let bounds = CGDisplayBounds(display)
    marks.append(DispatchTime.now().uptimeNanoseconds)
    var event: CGEvent? = CGEvent(
        mouseEventSource: source,
        mouseType: .mouseMoved,
        mouseCursorPosition: CGPoint(x: bounds.midX, y: bounds.midY),
        mouseButton: .left
    )
    marks.append(DispatchTime.now().uptimeNanoseconds)
    event?.setIntegerValueField(.mouseEventDeltaX, value: 1)
    event?.setIntegerValueField(.mouseEventDeltaY, value: 1)
    event?.flags = []
    marks.append(DispatchTime.now().uptimeNanoseconds)
    event = nil
    marks.append(DispatchTime.now().uptimeNanoseconds)
    let values = (0..<names.count).map { Double(marks[$0 + 1] - marks[$0]) / 1000 }
    for index in names.indices {
        totals[index] += values[index]
        maxima[index] = max(maxima[index], values[index])
    }
    if values.reduce(0, +) >= 2000 {
        let detail = names.indices.map { "\(names[$0])_us=\(Int(values[$0]))" }.joined(separator: " ")
        print("slow probe seq=\(sequence) \(detail)")
    }
    Thread.sleep(forTimeInterval: 0.008)
}
for index in names.indices {
    print("\(names[index]): n=\(count) mean_us=\(totals[index] / Double(count)) max_us=\(maxima[index])")
}
print("elapsed_s=\(Double(DispatchTime.now().uptimeNanoseconds - start) / 1e9)")
