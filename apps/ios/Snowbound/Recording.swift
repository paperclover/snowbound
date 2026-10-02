import AVFoundation
import UIKit

/// A recording `sb_view_play_request` asks to play: its sound file from `atMs`, and for a
/// video the AVI file whose JPEG pictures `frames` gives, `frameUs` apart.
struct PlayRequest: Decodable {
    struct Video: Decodable {
        let path: String
        let frames: [[Int]]
        let frameUs: UInt32
        let size: [Int]
    }
    let name: String
    let sound: String
    let video: Video?
    let atMs: UInt32
}

/// Record Audio and Record Video, stored as the desktop stores them: sound as mono PCM at
/// `sb_recording_rate`, which the page keeps as IMA ADPCM, and video as a movie from the
/// camera made into Motion JPEG AVI. Only one records at a time, and it keeps the page it
/// records on until its file is there, whether or not the page is still shown.
///
/// Audio records on with the screen locked or the app in the background, under the audio
/// background mode, as a lecture or meeting outlasts the screen's timeout; a call pauses it.
/// The camera stops in the background, so a video recording then ends and is saved.
final class Recorder: NSObject, AVCaptureFileOutputRecordingDelegate {
    /// The recording under way or being saved.
    private(set) static var current: Recorder?
    /// Posted when the current recording pauses, resumes, saves or ends.
    static let changed = Notification.Name("SnowboundRecordingChanged")

    let video: Bool
    let canvas: CanvasView
    private let folder: URL
    private var sound: AVAudioRecorder?
    private var capture: AVCaptureSession?
    private let output = AVCaptureMovieFileOutput()
    #if targetEnvironment(simulator)
    private var stub: StubCamera?
    #endif
    /// Stopped, its file being made and put on the page.
    private(set) var saving = false

    /// Whether video can be recorded here: with a camera, or the simulator's stand-in.
    static var canRecordVideo: Bool {
        #if targetEnvironment(simulator)
        true
        #else
        AVCaptureDevice.default(for: .video) != nil
        #endif
    }

    private init(video: Bool, canvas: CanvasView) {
        self.video = video
        self.canvas = canvas
        folder = FileManager.default.temporaryDirectory.appendingPathComponent("Recording")
        super.init()
    }

    /// Starts recording on `canvas` at its caret once the system allows the microphone, and
    /// for video the camera; `failed` says why it could not.
    static func start(video: Bool, on canvas: CanvasView, failed: @escaping (String, String) -> Void) {
        guard current == nil else { return }
        let title = video ? "Couldn't Record Video" : "Couldn't Record Audio"
        AVAudioApplication.requestRecordPermission { microphone in
            let begin = { (camera: Bool) in
                DispatchQueue.main.async {
                    guard current == nil else { return }
                    if video && !camera {
                        return failed(title, "Allow Snowbound to use the camera in Settings, Privacy & Security, Camera.")
                    }
                    // A video without the microphone records silent, as on the desktop.
                    if !video && !microphone {
                        return failed(
                            title, "Allow Snowbound to use the microphone in Settings, Privacy & Security, Microphone.")
                    }
                    let recorder = Recorder(video: video, canvas: canvas)
                    do {
                        try recorder.begin(microphone: microphone)
                    } catch {
                        recorder.end()
                        return failed(title, "Try recording again.")
                    }
                    guard canvas.startRecording(video: video) else {
                        recorder.end()
                        return failed(title, "Try recording again.")
                    }
                    current = recorder
                    NotificationCenter.default.post(name: changed, object: recorder)
                }
            }
            if video {
                #if targetEnvironment(simulator)
                begin(true)
                #else
                AVCaptureDevice.requestAccess(for: .video, completionHandler: begin)
                #endif
            } else {
                begin(true)
            }
        }
    }

    private func begin(microphone: Bool) throws {
        try? FileManager.default.removeItem(at: folder)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let session = AVAudioSession.sharedInstance()
        if microphone {
            try session.setCategory(.playAndRecord, mode: .default, options: [.defaultToSpeaker, .allowBluetoothHFP])
            try session.setActive(true)
            NotificationCenter.default.addObserver(
                self, selector: #selector(interrupted), name: AVAudioSession.interruptionNotification, object: session)
        }
        #if targetEnvironment(simulator)
        if video {
            stub = try StubCamera(movie: folder.appendingPathComponent("camera.mov"))
        }
        #endif
        if !video || isStubbed {
            guard microphone else { return }
            let settings: [String: Any] = [
                AVFormatIDKey: kAudioFormatLinearPCM, AVSampleRateKey: sb_recording_rate(), AVNumberOfChannelsKey: 1,
                AVLinearPCMBitDepthKey: 16, AVLinearPCMIsFloatKey: false, AVLinearPCMIsBigEndianKey: false,
            ]
            let sound = try AVAudioRecorder(url: soundFile, settings: settings)
            guard sound.record() else { throw CocoaError(.fileWriteUnknown) }
            self.sound = sound
            return
        }
        let capture = AVCaptureSession()
        capture.beginConfiguration()
        if capture.canSetSessionPreset(.vga640x480) { capture.sessionPreset = .vga640x480 }
        let devices = [AVCaptureDevice.default(for: .video)] + (microphone ? [AVCaptureDevice.default(for: .audio)] : [])
        for (index, device) in devices.enumerated() {
            guard let device, let input = try? AVCaptureDeviceInput(device: device), capture.canAddInput(input) else {
                // Without the microphone the video records silent; without the camera, nothing.
                if index == 0 { throw CocoaError(.fileWriteUnknown) }
                continue
            }
            capture.addInput(input)
        }
        guard capture.canAddOutput(output) else { throw CocoaError(.fileWriteUnknown) }
        capture.addOutput(output)
        capture.commitConfiguration()
        capture.startRunning()
        output.startRecording(to: movieFile, recordingDelegate: self)
        self.capture = capture
    }

    private var isStubbed: Bool {
        #if targetEnvironment(simulator)
        stub != nil
        #else
        false
        #endif
    }

    private var soundFile: URL { folder.appendingPathComponent("sound.wav") }
    private var movieFile: URL { folder.appendingPathComponent("camera.mov") }

    /// How far the recording has got, its pauses left out, and whether it is paused.
    var elapsed: (ms: Int64, paused: Bool)? { canvas.recording }

    /// Whether Pause works: always but for a camera before iOS 18.
    var pausable: Bool {
        if capture == nil { return true }
        if #available(iOS 18, *) { return true }
        return false
    }

    func pause(_ paused: Bool) {
        guard !saving, pausable else { return }
        if paused {
            sound?.pause()
        } else {
            sound?.record()
        }
        #if targetEnvironment(simulator)
        stub?.paused = paused
        #endif
        if #available(iOS 18, *), capture != nil {
            if paused { output.pauseRecording() } else { output.resumeRecording() }
        }
        canvas.pauseRecording(paused)
        NotificationCenter.default.post(name: Self.changed, object: self)
    }

    /// A call or another app's audio takes the microphone: the recording pauses, and goes
    /// on after where the system says it should.
    @objc private func interrupted(_ notification: Notification) {
        guard let raw = notification.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
            let type = AVAudioSession.InterruptionType(rawValue: raw)
        else { return }
        DispatchQueue.main.async { [self] in
            switch type {
            case .began where capture == nil: pause(true)
            case .ended:
                let options = (notification.userInfo?[AVAudioSessionInterruptionOptionKey] as? UInt)
                    .map(AVAudioSession.InterruptionOptions.init) ?? []
                if options.contains(.shouldResume), capture == nil {
                    try? AVAudioSession.sharedInstance().setActive(true)
                    pause(false)
                }
            default: break
            }
        }
    }

    /// Stop: the recording's file is made and goes on the page where it started.
    func stop() {
        guard !saving else { return }
        saving = true
        canvas.pauseRecording(true)
        NotificationCenter.default.post(name: Self.changed, object: self)
        if capture != nil {
            // The movie is saved once `fileOutput(_:didFinishRecordingTo:…)` hears so.
            output.stopRecording()
            return
        }
        sound?.stop()
        #if targetEnvironment(simulator)
        if let stub {
            stub.finish { [self] in convert(movie: stub.movie) }
            return
        }
        #endif
        save(try? Data(contentsOf: soundFile))
    }

    /// The camera stopped: at Stop, or as the app left the screen or a call took the camera.
    func fileOutput(
        _ output: AVCaptureFileOutput, didFinishRecordingTo url: URL, from connections: [AVCaptureConnection],
        error: Error?
    ) {
        let finished = (error as NSError?)?.userInfo[AVErrorRecordingSuccessfullyFinishedKey] as? Bool ?? (error == nil)
        DispatchQueue.main.async { [self] in
            saving = true
            canvas.pauseRecording(true)
            NotificationCenter.default.post(name: Self.changed, object: self)
            capture?.stopRunning()
            if finished { convert(movie: url) } else { save(nil) }
        }
    }

    /// Makes the AVI file of `movie`, with the sound recorded beside it where there is.
    private func convert(movie: URL) {
        let sound = self.sound == nil ? movie : soundFile
        // Finishing is not cut short by the app leaving the screen.
        let task = UIApplication.shared.beginBackgroundTask()
        Task.detached {
            let avi = await avi(pictures: movie, sound: sound)
            await MainActor.run {
                self.save(avi)
                UIApplication.shared.endBackgroundTask(task)
            }
        }
    }

    /// Puts `file` on the page, or with none forgets the recording.
    private func save(_ file: Data?) {
        canvas.finishRecording(file, video: video)
        end()
        Self.current = nil
        NotificationCenter.default.post(name: Self.changed, object: self)
    }

    private func end() {
        NotificationCenter.default.removeObserver(self)
        capture?.stopRunning()
        try? FileManager.default.removeItem(at: folder)
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }
}

/// The AVI file the desktop makes of a movie: the pictures of `pictures`' first video track
/// and the sound of `sound`'s first audio track, mono PCM at `sb_recording_rate`.
private func avi(pictures: URL, sound: URL) async -> Data? {
    let movie = sb_movie_new()
    let asset = AVURLAsset(url: pictures)
    await read(asset, .video, [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA]) { sample in
        let time = CMSampleBufferGetPresentationTimeStamp(sample)
        guard time.isValid, let buffer = CMSampleBufferGetImageBuffer(sample) else { return }
        CVPixelBufferLockBaseAddress(buffer, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(buffer, .readOnly) }
        guard let base = CVPixelBufferGetBaseAddress(buffer) else { return }
        sb_movie_picture(
            movie, UInt64(max(0, time.seconds) * 1_000_000), base.assumingMemoryBound(to: UInt8.self),
            UInt32(CVPixelBufferGetWidth(buffer)), UInt32(CVPixelBufferGetHeight(buffer)),
            CVPixelBufferGetBytesPerRow(buffer))
    }
    let pcm: [String: Any] = [
        AVFormatIDKey: kAudioFormatLinearPCM, AVSampleRateKey: sb_recording_rate(), AVNumberOfChannelsKey: 1,
        AVLinearPCMBitDepthKey: 16, AVLinearPCMIsFloatKey: false, AVLinearPCMIsBigEndianKey: false,
        AVLinearPCMIsNonInterleaved: false,
    ]
    // A movie without sound has no sound track to read.
    await read(AVURLAsset(url: sound), .audio, pcm) { sample in
        guard let buffer = CMSampleBufferGetDataBuffer(sample) else { return }
        let length = CMBlockBufferGetDataLength(buffer)
        var bytes = [UInt8](repeating: 0, count: length)
        if CMBlockBufferCopyDataBytes(buffer, atOffset: 0, dataLength: length, destination: &bytes) == noErr {
            sb_movie_sound(movie, bytes, length)
        }
    }
    let duration = (try? await asset.load(.duration))?.seconds ?? 0
    var length = 0
    guard let bytes = sb_movie_finish(movie, UInt64(max(0, duration) * 1_000_000), &length) else { return nil }
    defer { sb_bytes_free(bytes, length) }
    return Data(bytes: bytes, count: length)
}

/// Reads the first track of `kind` in `asset`, decoded as `settings` ask, a sample at a time.
private func read(
    _ asset: AVURLAsset, _ kind: AVMediaType, _ settings: [String: Any], _ sample: (CMSampleBuffer) -> Void
) async {
    guard let track = try? await asset.loadTracks(withMediaType: kind).first,
        let reader = try? AVAssetReader(asset: asset)
    else { return }
    let output = AVAssetReaderTrackOutput(track: track, outputSettings: settings)
    guard reader.canAdd(output) else { return }
    reader.add(output)
    guard reader.startReading() else { return }
    while let next = output.copyNextSampleBuffer() {
        sample(next)
    }
}

#if targetEnvironment(simulator)
/// The simulator's stand-in for a camera, which it has none of: moving colour bars and the
/// time written as a movie, 15 pictures a second, while the microphone records beside it.
private final class StubCamera {
    let movie: URL
    private let writer: AVAssetWriter
    private let input: AVAssetWriterInput
    private let adaptor: AVAssetWriterInputPixelBufferAdaptor
    private var timer: Timer?
    private var frame: Int64 = 0
    var paused = false

    init(movie: URL) throws {
        self.movie = movie
        writer = try AVAssetWriter(outputURL: movie, fileType: .mov)
        input = AVAssetWriterInput(
            mediaType: .video,
            outputSettings: [AVVideoCodecKey: AVVideoCodecType.h264, AVVideoWidthKey: 640, AVVideoHeightKey: 480])
        input.expectsMediaDataInRealTime = true
        adaptor = AVAssetWriterInputPixelBufferAdaptor(
            assetWriterInput: input,
            sourcePixelBufferAttributes: [
                kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
                kCVPixelBufferWidthKey as String: 640, kCVPixelBufferHeightKey as String: 480,
            ])
        writer.add(input)
        guard writer.startWriting() else { throw writer.error ?? CocoaError(.fileWriteUnknown) }
        writer.startSession(atSourceTime: .zero)
        timer = Timer.scheduledTimer(withTimeInterval: 1.0 / 15, repeats: true) { [weak self] _ in self?.shoot() }
    }

    private func shoot() {
        guard !paused, input.isReadyForMoreMediaData, let pool = adaptor.pixelBufferPool else { return }
        var made: CVPixelBuffer?
        CVPixelBufferPoolCreatePixelBuffer(nil, pool, &made)
        guard let buffer = made else { return }
        CVPixelBufferLockBaseAddress(buffer, [])
        let context = CGContext(
            data: CVPixelBufferGetBaseAddress(buffer), width: 640, height: 480, bitsPerComponent: 8,
            bytesPerRow: CVPixelBufferGetBytesPerRow(buffer), space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)
        if let context {
            let colors: [UIColor] = [.systemYellow, .systemTeal, .systemGreen, .systemPink, .systemRed, .systemBlue]
            for bar in 0..<8 {
                context.setFillColor(colors[(bar + Int(frame / 5)) % colors.count].cgColor)
                context.fill(CGRect(x: bar * 80, y: 0, width: 80, height: 480))
            }
            UIGraphicsPushContext(context)
            context.translateBy(x: 0, y: 480)
            context.scaleBy(x: 1, y: -1)
            let seconds = Double(frame) / 15
            NSString(string: String(format: "%.1f s", seconds)).draw(
                at: CGPoint(x: 24, y: 24),
                withAttributes: [.font: UIFont.monospacedDigitSystemFont(ofSize: 72, weight: .bold), .foregroundColor: UIColor.black])
            UIGraphicsPopContext()
        }
        CVPixelBufferUnlockBaseAddress(buffer, [])
        adaptor.append(buffer, withPresentationTime: CMTime(value: frame, timescale: 15))
        frame += 1
    }

    func finish(_ done: @escaping () -> Void) {
        timer?.invalidate()
        input.markAsFinished()
        writer.endSession(atSourceTime: CMTime(value: frame, timescale: 15))
        writer.finishWriting { DispatchQueue.main.async(execute: done) }
    }
}
#endif

/// Plays a recording: its sound, and a video's pictures in time with it.
final class Playback {
    let request: PlayRequest
    let player: AVPlayer
    private let pictures: Data?
    private var shown = -1

    init(_ request: PlayRequest) {
        self.request = request
        player = AVPlayer(url: URL(fileURLWithPath: request.sound))
        pictures = request.video.flatMap { try? Data(contentsOf: URL(fileURLWithPath: $0.path), options: .alwaysMapped) }
        try? AVAudioSession.sharedInstance().setCategory(.playback, mode: .spokenAudio)
        try? AVAudioSession.sharedInstance().setActive(true)
        player.seek(to: CMTime(value: CMTimeValue(request.atMs), timescale: 1000), toleranceBefore: .zero, toleranceAfter: .zero)
        player.play()
    }

    deinit {
        player.pause()
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }

    /// Whether the player could not open the sound, as with OneNote's own Windows Media.
    var failed: Bool { player.currentItem?.status == .failed }

    var ms: Int64 { Int64(max(0, player.currentTime().seconds) * 1000) }

    var durationMs: Int64? {
        guard let duration = player.currentItem?.duration, duration.isNumeric else { return nil }
        return Int64(duration.seconds * 1000)
    }

    var playing: Bool { player.rate != 0 }

    func toggle() {
        if playing {
            player.pause()
        } else {
            if let durationMs, ms >= durationMs - 50 { player.seek(to: .zero) }
            player.play()
        }
    }

    /// The video's picture now, when it changed since last asked.
    func picture() -> UIImage? {
        guard let video = request.video, let pictures, !video.frames.isEmpty else { return nil }
        let index = min(Int(ms * 1000 / Int64(max(1, video.frameUs))), video.frames.count - 1)
        guard index != shown else { return nil }
        shown = index
        let range = video.frames[index]
        guard range.count == 2, range[0] < range[1], range[1] <= pictures.count else { return nil }
        return UIImage(data: pictures.subdata(in: range[0]..<range[1]))
    }
}

/// The bar over the page's foot while something records or plays, as OneNote's Recording and
/// Playback tabs: the clock, Pause, and Stop; a video's pictures above as it plays.
final class MediaBar: UIView {
    private let dot = UIImageView(image: UIImage(systemName: "record.circle.fill"))
    private let label = UILabel()
    private let pause = UIButton(configuration: .plain())
    private let stop = UIButton(configuration: .plain())
    let picture = UIImageView()
    var onPause: (() -> Void)?
    var onStop: (() -> Void)?

    init() {
        super.init(frame: .zero)
        let blur = UIVisualEffectView(effect: UIBlurEffect(style: .systemThickMaterial))
        blur.translatesAutoresizingMaskIntoConstraints = false
        blur.layer.cornerRadius = 14
        blur.clipsToBounds = true
        addSubview(blur)
        layer.shadowOpacity = 0.15
        layer.shadowRadius = 8
        layer.shadowOffset = CGSize(width: 0, height: 2)
        dot.tintColor = .systemRed
        dot.setContentHuggingPriority(.required, for: .horizontal)
        label.font = .monospacedDigitSystemFont(ofSize: UIFont.preferredFont(forTextStyle: .subheadline).pointSize, weight: .medium)
        label.adjustsFontForContentSizeCategory = true
        label.lineBreakMode = .byTruncatingMiddle
        label.accessibilityTraits.insert(.updatesFrequently)
        stop.configuration?.image = UIImage(systemName: "stop.fill")
        stop.accessibilityLabel = "Stop"
        stop.addAction(UIAction { [weak self] _ in self?.onStop?() }, for: .primaryActionTriggered)
        pause.addAction(UIAction { [weak self] _ in self?.onPause?() }, for: .primaryActionTriggered)
        for button in [pause, stop] { button.setContentHuggingPriority(.required, for: .horizontal) }
        picture.contentMode = .scaleAspectFit
        picture.backgroundColor = .black
        picture.layer.cornerRadius = 6
        picture.clipsToBounds = true
        picture.isHidden = true
        let controls = UIStackView(arrangedSubviews: [dot, label, pause, stop])
        controls.spacing = 8
        controls.alignment = .center
        let stack = UIStackView(arrangedSubviews: [picture, controls])
        stack.axis = .vertical
        stack.spacing = 8
        stack.translatesAutoresizingMaskIntoConstraints = false
        blur.contentView.addSubview(stack)
        NSLayoutConstraint.activate([
            blur.leadingAnchor.constraint(equalTo: leadingAnchor),
            blur.trailingAnchor.constraint(equalTo: trailingAnchor),
            blur.topAnchor.constraint(equalTo: topAnchor),
            blur.bottomAnchor.constraint(equalTo: bottomAnchor),
            stack.leadingAnchor.constraint(equalTo: blur.contentView.leadingAnchor, constant: 14),
            stack.trailingAnchor.constraint(equalTo: blur.contentView.trailingAnchor, constant: -6),
            stack.topAnchor.constraint(equalTo: blur.contentView.topAnchor, constant: 8),
            stack.bottomAnchor.constraint(equalTo: blur.contentView.bottomAnchor, constant: -8),
            picture.widthAnchor.constraint(lessThanOrEqualToConstant: 320),
        ])
        // Below required, as the stack gives the picture no height while it is hidden.
        let aspect = picture.heightAnchor.constraint(equalTo: picture.widthAnchor, multiplier: 3.0 / 4)
        aspect.priority = .defaultHigh
        aspect.isActive = true
        isHidden = true
    }

    required init?(coder: NSCoder) { fatalError() }

    /// Shows `text`, the dot while recording, and Pause or Resume (or Play) where it works.
    func show(_ text: String, recording: Bool, paused: Bool?, playing: Bool = false) {
        isHidden = false
        label.text = text
        dot.isHidden = !recording
        dot.alpha = paused == true ? 0.4 : 1
        pause.isHidden = paused == nil
        stop.isHidden = false
        let resume = paused == true
        pause.configuration?.image = UIImage(systemName: resume ? (playing ? "play.fill" : "record.circle") : "pause.fill")
        pause.accessibilityLabel = resume ? (playing ? "Play" : "Resume") : "Pause"
    }

    /// Saving: only the clock's place says so.
    func showSaving(_ text: String) {
        show(text, recording: false, paused: nil)
        stop.isHidden = true
    }

    /// `ms` as a clock: minutes and seconds, hours when there are some.
    static func clock(_ ms: Int64) -> String {
        let seconds = ms / 1000
        return seconds >= 3600
            ? String(format: "%d:%02d:%02d", seconds / 3600, seconds / 60 % 60, seconds % 60)
            : String(format: "%d:%02d", seconds / 60, seconds % 60)
    }
}
