<?php
// app/Support/EasypanelException.php
namespace App\Support;

use Illuminate\Http\Client\Response;
use RuntimeException;

class EasypanelException extends RuntimeException
{
    public static function fromResponse(Response $response): self
    {
        $status = $response->status();
        $message = $response->json('message') ?? $response->json('error') ?? $response->reason();

        if ($status === 401) {
            $message = 'Token tidak valid atau kadaluarsa (401).';
        }

        return new self("[{$status}] {$message}");
    }
}
