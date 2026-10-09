// The tests of the Koetama client (no test framework: a check list, like the repo's Python tests).
//
//     cd examples/mods/valheim/Tests
//     dotnet run -c Release                 unit tests: JSON, helpers, the client against a fake Koetama
//     dotnet run -c Release -- e2e          against the REAL Koetama (EndToEnd.cs)
using System;

namespace Koetama.Tests
{
    internal static class Program
    {
        private static int checks, failed;

        public static void Check(bool ok, string what)
        {
            checks++;
            if (!ok) failed++;
            Console.WriteLine((ok ? "ok   " : "FAIL ") + what);
        }

        private static int Main(string[] args)
        {
            Console.OutputEncoding = System.Text.Encoding.UTF8;
            try
            {
                if (args.Length > 0 && args[0] == "e2e")
                {
                    EndToEnd.Run();
                }
                else
                {
                    JsonTests.Run();
                    HelperTests.Run();
                    FakeServerTests.Run();
                }
            }
            catch (Exception e)
            {
                Check(false, "unexpected: " + e);
            }
            Console.WriteLine();
            Console.WriteLine(checks + " checks, " + failed + " failed");
            return failed == 0 ? 0 : 1;
        }
    }
}
