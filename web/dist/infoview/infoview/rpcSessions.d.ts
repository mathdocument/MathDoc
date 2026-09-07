import { RpcSessionAtPos } from '@leanprover/infoview-api';
import * as React from 'react';
import type { TextDocumentPositionParams } from 'vscode-languageserver-protocol';
import { DocumentPosition } from './util';
/**
 * Provides a {@link RpcSessionsContext} to the children.
 * The {@link RpcSessions} object stored there manages RPC sessions in the Lean server.
 */
export declare function WithRpcSessions({ children }: {
    children: React.ReactNode;
}): import("react/jsx-runtime").JSX.Element;
export declare function useRpcSessionAtTdpp(pos: TextDocumentPositionParams): RpcSessionAtPos;
export declare function useRpcSessionAtPos(pos: DocumentPosition): RpcSessionAtPos;
/** @deprecated use {@link useRpcSession} instead */
export declare const RpcContext: React.Context<RpcSessionAtPos>;
/**
 * Retrieve an RPC session at {@link EnvPosContext},
 * if the context is set.
 * Otherwise return a dummy session that throws on any RPC call.
 */
export declare function useRpcSession(): RpcSessionAtPos;
