package ai.ridge.translator;

import java.io.BufferedReader;
import java.io.FileReader;

/**
 * P7.2 / J-14：模拟器 RSS 与调用时延基线。
 *
 * 每次迭代都走完整的 create → lookup → detect → close，避免把"复用句柄"的
 * 热路径当成冷启动。所有数字都标"模拟器"，不外推到物理设备或真实 NMT。
 *
 * 输出一行 JSON，由宿主脚本汇总 p50/p95；不在这里做统计，避免口径藏在代码里。
 */
public final class PerfHarness {
    public static void main(String[] args) throws Exception {
        String dictDir = args.length > 0 ? args[0] : "/data/local/tmp/dictpkg";
        int iterations = args.length > 1 ? Integer.parseInt(args[1]) : 35;
        if (iterations < 30) {
            System.out.println("{\"error\":\"iterations must be >= 30\"}");
            System.exit(2);
        }

        String config = "{"
                + "\"protocol_version\":1,"
                + "\"data_dir\":\"/data/local/tmp/p7perf\","
                + "\"dict_package\":\"" + dictDir + "\","
                + "\"term_policy\":\"warn\"}";

        // 预热：不计入样本，避免第一次类加载/JNI 解析污染 p50。
        for (int i = 0; i < 3; i++) {
            String h = handle(NativeBridge.nativeCreate(config));
            NativeBridge.nativeCall(h, "lookup", "{\"term\":\"你好\",\"limit\":5}");
            NativeBridge.nativeCall(h, "detect", "{\"text\":\"你好世界\"}");
            NativeBridge.nativeClose(h);
        }

        long[] createNs = new long[iterations];
        long[] lookupNs = new long[iterations];
        long[] detectNs = new long[iterations];
        long[] closeNs = new long[iterations];
        int[] rssAfterCreate = new int[iterations];
        int[] rssAfterLookup = new int[iterations];
        int[] rssAfterClose = new int[iterations];
        int peakObserved = 0;

        for (int i = 0; i < iterations; i++) {
            long t0 = System.nanoTime();
            String h = handle(NativeBridge.nativeCreate(config));
            long t1 = System.nanoTime();
            createNs[i] = t1 - t0;
            rssAfterCreate[i] = statusKb("VmRSS");

            String lk = NativeBridge.nativeCall(h, "lookup", "{\"term\":\"你好\",\"limit\":5}");
            long t2 = System.nanoTime();
            lookupNs[i] = t2 - t1;
            rssAfterLookup[i] = statusKb("VmRSS");
            if (lk == null || !lk.contains("\"ok\":true")) {
                System.out.println("{\"error\":\"lookup failed at iteration " + i + "\"}");
                System.exit(3);
            }

            String dt = NativeBridge.nativeCall(h, "detect", "{\"text\":\"你好世界\"}");
            long t3 = System.nanoTime();
            detectNs[i] = t3 - t2;
            if (dt == null || !dt.contains("\"ok\":true")) {
                System.out.println("{\"error\":\"detect failed at iteration " + i + "\"}");
                System.exit(4);
            }

            NativeBridge.nativeClose(h);
            long t4 = System.nanoTime();
            closeNs[i] = t4 - t3;
            rssAfterClose[i] = statusKb("VmRSS");
            peakObserved = Math.max(peakObserved, statusKb("VmHWM"));
        }

        StringBuilder out = new StringBuilder(4096);
        out.append("{\"stage\":\"J-14\",\"device\":\"gms2/34/x86_64\",\"iterations\":")
                .append(iterations)
                .append(",\"create_ms\":").append(ms(createNs))
                .append(",\"lookup_ms\":").append(ms(lookupNs))
                .append(",\"detect_ms\":").append(ms(detectNs))
                .append(",\"close_ms\":").append(ms(closeNs))
                .append(",\"rss_kb_after_create\":").append(nums(rssAfterCreate))
                .append(",\"rss_kb_after_lookup\":").append(nums(rssAfterLookup))
                .append(",\"rss_kb_after_close\":").append(nums(rssAfterClose))
                .append(",\"vmhwm_kb\":").append(statusKb("VmHWM"))
                .append(",\"peak_observed_kb\":").append(peakObserved)
                .append(",\"rss_source\":\"proc/self/status\"}");
        System.out.println(out);
    }

    /** 与 Harness 相同的 handle 提取，不依赖 JSON 库。 */
    private static String handle(String created) {
        String marker = "\"handle\":\"";
        int i = created.indexOf(marker);
        if (i < 0) {
            throw new IllegalStateException("create 失败: " + created);
        }
        int from = i + marker.length();
        int to = created.indexOf('"', from);
        return created.substring(from, to);
    }

    private static String ms(long[] ns) {
        StringBuilder sb = new StringBuilder("[");
        for (int i = 0; i < ns.length; i++) {
            if (i > 0) sb.append(',');
            sb.append(String.format(java.util.Locale.ROOT, "%.3f", ns[i] / 1_000_000.0));
        }
        return sb.append(']').toString();
    }

    private static String nums(int[] values) {
        StringBuilder sb = new StringBuilder("[");
        for (int i = 0; i < values.length; i++) {
            if (i > 0) sb.append(',');
            sb.append(values[i]);
        }
        return sb.append(']').toString();
    }

    /** 读 /proc/self/status 的 kB 字段；读不到返回 -1（不猜）。 */
    private static int statusKb(String key) {
        try (BufferedReader reader = new BufferedReader(new FileReader("/proc/self/status"))) {
            String line;
            while ((line = reader.readLine()) != null) {
                if (line.startsWith(key + ":")) {
                    String rest = line.substring(key.length() + 1).trim();
                    return Integer.parseInt(rest.split("\\s+")[0]);
                }
            }
        } catch (Exception ignored) {
            return -1;
        }
        return -1;
    }

    private PerfHarness() {}
}
